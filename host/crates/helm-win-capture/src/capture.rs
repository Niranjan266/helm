//! Windows.Graphics.Capture: monitor / window capture into D3D11 textures.
//!
//! A `GraphicsCaptureItem` is created for a monitor (`IGraphicsCaptureItemInterop::CreateForMonitor`)
//! or a window (`CreateForWindow`). Frames come from a *free-threaded*
//! `Direct3D11CaptureFramePool`, so `FrameArrived` fires on a system thread-pool thread — no
//! dispatcher queue or message loop is needed. The callback gets the frame's `ID3D11Texture2D`
//! (valid only during the call) and its `SystemRelativeTime` (QPC time base, 100 ns units).
//!
//! The WinRT objects here (item, pool, session) are agile, so `CaptureSession` can be created
//! on one thread and stopped on another.
//! When the source changes size, the pool is recreated with the new size.

use std::sync::Mutex;

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use crate::config::StreamSource;
use crate::d3d::D3dDevice;
use crate::error::{Context, Error, Result};
use crate::monitors::find_monitor;

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const POOL_BUFFERS: i32 = 2;

/// One captured frame, borrowed for the duration of the callback.
pub struct CapturedFrame<'a> {
    /// BGRA texture owned by the frame pool. Copy / convert it before returning.
    pub texture: &'a ID3D11Texture2D,
    /// Size of the captured content. Can be smaller than the texture right after a resize.
    pub content_width: u32,
    pub content_height: u32,
    pub texture_width: u32,
    pub texture_height: u32,
    /// `Direct3D11CaptureFrame.SystemRelativeTime` in 100 ns units (QueryPerformanceCounter
    /// time base). This is DWM's *target composition time* and is usually a few ms in the
    /// future when the callback runs: good for pacing, not for latency measurement.
    pub system_relative_time_100ns: i64,
}

/// True when Windows.Graphics.Capture is available (Windows 10 1803+).
pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

fn item_for_monitor(h: HMONITOR) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .ctx("GraphicsCaptureItem factory (IGraphicsCaptureItemInterop)")?;
    // SAFETY: valid monitor handle from EnumDisplayMonitors.
    unsafe { interop.CreateForMonitor(h) }.ctx("IGraphicsCaptureItemInterop::CreateForMonitor")
}

fn item_for_window(hwnd: isize) -> Result<GraphicsCaptureItem> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .ctx("GraphicsCaptureItem factory (IGraphicsCaptureItemInterop)")?;
    // SAFETY: the API validates the handle and fails for invalid / non-capturable windows.
    unsafe { interop.CreateForWindow(HWND(hwnd as *mut _)) }.ctx("IGraphicsCaptureItemInterop::CreateForWindow")
}

/// Create the capture item for a stream source.
pub fn create_item(source: &StreamSource) -> Result<GraphicsCaptureItem> {
    if !is_supported() {
        return Err(Error::Unsupported("Windows.Graphics.Capture is not supported on this system".into()));
    }
    match source {
        StreamSource::PrimaryMonitor => item_for_monitor(find_monitor(None)?.handle),
        StreamSource::Monitor(id) => item_for_monitor(find_monitor(Some(id))?.handle),
        StreamSource::Window(h) => item_for_window(*h),
    }
}

/// Size of a capture item in pixels.
pub fn item_size(item: &GraphicsCaptureItem) -> Result<(u32, u32)> {
    let s = item.Size().ctx("GraphicsCaptureItem::Size")?;
    Ok((s.Width.max(0) as u32, s.Height.max(0) as u32))
}

/// A running capture. Dropping it (or calling [`CaptureSession::stop`]) ends the capture.
pub struct CaptureSession {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frame_token: i64,
    closed_token: Option<i64>,
    stopped: bool,
    /// Whether the yellow capture border could be turned off (Windows 11+).
    pub border_disabled: bool,
    /// Whether the system-side frame-rate limit (`MinUpdateInterval`, Windows 11 24H2+) was set.
    pub min_update_interval_set: bool,
}

/// `IDirect3DDevice` is an agile WinRT object; this wrapper lets the frame handler own it.
struct AgileDevice(IDirect3DDevice);
// SAFETY: WinRT Direct3D device wrappers are agile (callable from any thread).
unsafe impl Send for AgileDevice {}

impl AgileDevice {
    // A method (not `.0`) so closures capture the whole Send wrapper, not the inner field.
    fn get(&self) -> &IDirect3DDevice {
        &self.0
    }
}

struct HandlerState<F> {
    on_frame: F,
    pool_size: SizeInt32,
}

impl CaptureSession {
    /// Start capturing `item`. `on_frame` runs on a thread-pool thread for every frame;
    /// `on_closed` runs once if the source goes away (window closed, monitor unplugged).
    /// `min_interval_100ns` asks Windows not to deliver frames faster than that (best effort).
    pub fn start<F, C>(
        device: &D3dDevice,
        item: GraphicsCaptureItem,
        min_interval_100ns: Option<i64>,
        on_frame: F,
        on_closed: C,
    ) -> Result<Self>
    where
        F: FnMut(&CapturedFrame) + Send + 'static,
        C: Fn() + Send + 'static,
    {
        let size = item.Size().ctx("GraphicsCaptureItem::Size")?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&device.winrt, PIXEL_FORMAT, POOL_BUFFERS, size)
            .ctx("Direct3D11CaptureFramePool::CreateFreeThreaded")?;

        let winrt_device = AgileDevice(device.winrt.clone());
        let state = Mutex::new(HandlerState { on_frame, pool_size: size });
        let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
            let Ok(pool) = pool.ok() else { return Ok(()) };
            let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
            // Errors here are swallowed: returning them to WinRT would not help anyone.
            let _ = handle_frame(pool, winrt_device.get(), &mut st);
            Ok(())
        });
        let frame_token = pool.FrameArrived(&handler).ctx("Direct3D11CaptureFramePool::FrameArrived")?;

        let closed_handler = TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
            on_closed();
            Ok(())
        });
        let closed_token = item.Closed(&closed_handler).ok();

        let session = pool.CreateCaptureSession(&item).ctx("CreateCaptureSession")?;
        // Cursor is sent separately (protocol), and IsCursorCaptureEnabled exists since 2004.
        let _ = session.SetIsCursorCaptureEnabled(false);
        // Yellow border off: Windows 11 only; older builds throw, which is fine.
        let border_disabled = session.SetIsBorderRequired(false).is_ok();
        let min_update_interval_set = match min_interval_100ns {
            Some(d) if d > 0 => session.SetMinUpdateInterval(TimeSpan { Duration: d }).is_ok(),
            _ => false,
        };
        session.StartCapture().ctx("GraphicsCaptureSession::StartCapture")?;
        Ok(Self {
            item,
            pool,
            session,
            frame_token,
            closed_token,
            stopped: false,
            border_disabled,
            min_update_interval_set,
        })
    }

    pub fn item(&self) -> &GraphicsCaptureItem {
        &self.item
    }

    /// Stop the capture. Idempotent. A callback that is already running may still finish.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = self.pool.RemoveFrameArrived(self.frame_token);
        if let Some(t) = self.closed_token.take() {
            let _ = self.item.RemoveClosed(t);
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_frame<F: FnMut(&CapturedFrame)>(
    pool: &Direct3D11CaptureFramePool,
    device: &IDirect3DDevice,
    st: &mut HandlerState<F>,
) -> windows::core::Result<()> {
    let frame = pool.TryGetNextFrame()?;
    let content = frame.ContentSize()?;
    let time = frame.SystemRelativeTime()?;
    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    // SAFETY: the surface of a D3D11 frame pool is backed by an ID3D11Texture2D.
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: valid texture, out-pointer to a local.
    unsafe { texture.GetDesc(&mut desc) };
    (st.on_frame)(&CapturedFrame {
        texture: &texture,
        content_width: content.Width.max(0) as u32,
        content_height: content.Height.max(0) as u32,
        texture_width: desc.Width,
        texture_height: desc.Height,
        system_relative_time_100ns: time.Duration,
    });
    drop(texture);
    let _ = frame.Close();
    if (content.Width != st.pool_size.Width || content.Height != st.pool_size.Height)
        && content.Width > 0
        && content.Height > 0
    {
        pool.Recreate(device, PIXEL_FORMAT, POOL_BUFFERS, content)?;
        st.pool_size = content;
    }
    Ok(())
}
