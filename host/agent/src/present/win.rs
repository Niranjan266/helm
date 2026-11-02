//! This PC's presentation backend: foreground window, PowerPoint automation (late-bound
//! `IDispatch` on the running `PowerPoint.Application`), and the laser overlay: one
//! topmost, click-through, layered window per chosen monitor, drawn by `overlay::Laser` on
//! its own thread.

use std::ptr::null_mut;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::System::Com::{
    CLSIDFromProgID, COINIT_MULTITHREADED, CoInitializeEx, DISPATCH_FLAGS, DISPATCH_METHOD,
    DISPATCH_PROPERTYGET, DISPPARAMS, IDispatch,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::GetActiveObject;
use windows::Win32::System::Variant::{
    VAR_CHANGE_FLAGS, VARIANT, VT_BSTR, VT_DISPATCH, VT_I4, VariantChangeType, VariantClear,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HTTRANSPARENT,
    HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, TranslateMessage,
    UPDATELAYEREDWINDOWINFO, ULW_ALPHA, UpdateLayeredWindowIndirect, WM_NCHITTEST, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::{GUID, IUnknown, Interface, PCWSTR, w};

use super::overlay::{IRect, Laser};
use super::{PresentBackend, PresentHub, SlideInfo, clean_notes};
use crate::platform::{ForegroundInfo, PResult, Rect};
use crate::router::RpcError;

/// The overlay hides itself after this long without `lp` frames.
const IDLE_HIDE: Duration = Duration::from_secs(3);
/// About one frame at 60 Hz while the trail fades.
const FRAME: Duration = Duration::from_millis(16);

/// The hub for this PC (one overlay, shared by all sessions).
pub(crate) fn hub() -> Arc<PresentHub> {
    static HUB: OnceLock<Arc<PresentHub>> = OnceLock::new();
    HUB.get_or_init(|| Arc::new(PresentHub::new(Arc::new(WinPresent))))
        .clone()
}

struct WinPresent;

impl PresentBackend for WinPresent {
    fn foreground(&self) -> PResult<Option<ForegroundInfo>> {
        helm_win_sysmon::foreground::current().map_err(crate::platform::sysmon_err)
    }
    fn powerpoint(&self) -> PResult<Option<SlideInfo>> {
        powerpoint_info()
    }
    fn laser_show(&self, rect: Rect, x: f32, y: f32, trail: bool) -> PResult<()> {
        overlay_send(Cmd::Show { rect, x, y, trail })
    }
    fn laser_hide(&self) -> PResult<()> {
        overlay_send(Cmd::Hide)
    }
}

// ---------------------------------------------------------------- PowerPoint automation

const LOCALE_USER_DEFAULT: u32 = 0x0400;
const PP_PLACEHOLDER_BODY: i32 = 2;

/// A VARIANT that is cleared when dropped.
struct Var(VARIANT);

impl Drop for Var {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}

impl Var {
    fn i4(n: i32) -> Var {
        let mut v = VARIANT::default();
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_I4;
            inner.Anonymous.lVal = n;
        }
        Var(v)
    }

    fn dispatch(&self) -> windows::core::Result<IDispatch> {
        unsafe {
            let inner = &*self.0.Anonymous.Anonymous;
            if inner.vt == VT_DISPATCH
                && let Some(d) = inner.Anonymous.pdispVal.as_ref()
            {
                return Ok(d.clone());
            }
        }
        Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80020005u32 as i32, // DISP_E_TYPEMISMATCH
        )))
    }

    fn convert(&self, vt: windows::Win32::System::Variant::VARENUM) -> windows::core::Result<Var> {
        let mut out = Var(VARIANT::default());
        unsafe { VariantChangeType(&mut out.0, &self.0, VAR_CHANGE_FLAGS(0), vt)? };
        Ok(out)
    }

    fn int(&self) -> windows::core::Result<i32> {
        let v = self.convert(VT_I4)?;
        Ok(unsafe { v.0.Anonymous.Anonymous.Anonymous.lVal })
    }

    fn string(&self) -> windows::core::Result<String> {
        let v = self.convert(VT_BSTR)?;
        Ok(unsafe { v.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string() })
    }
}

fn call(
    obj: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    arg: Option<Var>,
) -> windows::core::Result<Var> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let names = [PCWSTR(wide.as_ptr())];
    let mut id = 0i32;
    unsafe {
        obj.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, LOCALE_USER_DEFAULT, &mut id)?;
    }
    let mut args: Vec<VARIANT> = Vec::new();
    // The Var wrapper keeps ownership; DISPPARAMS only borrows the bits.
    let holder = arg;
    if let Some(a) = &holder {
        args.push(unsafe { std::ptr::read(&a.0) });
    }
    let params = DISPPARAMS {
        rgvarg: if args.is_empty() { null_mut() } else { args.as_mut_ptr() },
        rgdispidNamedArgs: null_mut(),
        cArgs: args.len() as u32,
        cNamedArgs: 0,
    };
    let mut result = Var(VARIANT::default());
    let r = unsafe {
        obj.Invoke(
            id,
            &GUID::zeroed(),
            LOCALE_USER_DEFAULT,
            flags,
            &params,
            Some(&mut result.0),
            None,
            None,
        )
    };
    // `args` holds bitwise copies of `holder`: forget them so only `holder` clears.
    for a in args {
        std::mem::forget(a);
    }
    drop(holder);
    r.map(|_| result)
}

fn get(obj: &IDispatch, name: &str) -> windows::core::Result<Var> {
    call(obj, name, DISPATCH_PROPERTYGET | DISPATCH_METHOD, None)
}

fn get_obj(obj: &IDispatch, name: &str) -> windows::core::Result<IDispatch> {
    get(obj, name)?.dispatch()
}

fn item(obj: &IDispatch, index: i32) -> windows::core::Result<IDispatch> {
    call(obj, "Item", DISPATCH_PROPERTYGET | DISPATCH_METHOD, Some(Var::i4(index)))?.dispatch()
}

fn com_init() {
    // Pool threads may already be initialised (either model); both are fine for an
    // out-of-process server, so the result is ignored and never uninitialised.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn running_powerpoint() -> PResult<IDispatch> {
    com_init();
    let clsid = unsafe { CLSIDFromProgID(w!("PowerPoint.Application")) }
        .map_err(|_| RpcError::unsupported("PowerPoint is not installed"))?;
    let mut unk: Option<IUnknown> = None;
    unsafe { GetActiveObject(&clsid, None, &mut unk) }
        .map_err(|_| RpcError::unsupported("PowerPoint is not running"))?;
    unk.ok_or_else(|| RpcError::unsupported("PowerPoint is not running"))?
        .cast::<IDispatch>()
        .map_err(|e| RpcError::unsupported(format!("PowerPoint cannot be automated: {e}")))
}

fn com_err(e: windows::core::Error) -> RpcError {
    // RPC_E_CALL_REJECTED / RPC_E_SERVERCALL_RETRYLATER: a dialog is open or PowerPoint is busy.
    let code = e.code().0 as u32;
    if code == 0x8001_0001 || code == 0x8001_010A {
        RpcError::internal("PowerPoint is busy (is a dialog open?)")
    } else {
        RpcError::internal(format!("PowerPoint automation failed: {e}"))
    }
}

fn slide_notes(slide: &IDispatch) -> windows::core::Result<String> {
    let shapes = get_obj(&get_obj(slide, "NotesPage")?, "Shapes")?;
    let placeholders = get_obj(&shapes, "Placeholders")?;
    let n = get(&placeholders, "Count")?.int()?;
    for i in 1..=n {
        let shape = item(&placeholders, i)?;
        let kind = get(&get_obj(&shape, "PlaceholderFormat")?, "Type")?.int()?;
        if kind == PP_PLACEHOLDER_BODY && get(&shape, "HasTextFrame")?.int()? != 0 {
            let text = get(&get_obj(&get_obj(&shape, "TextFrame")?, "TextRange")?, "Text")?;
            return Ok(clean_notes(&text.string()?));
        }
    }
    Ok(String::new())
}

fn powerpoint_info() -> PResult<Option<SlideInfo>> {
    let app = running_powerpoint()?;
    (|| -> windows::core::Result<Option<SlideInfo>> {
        let shows = get_obj(&app, "SlideShowWindows")?;
        if get(&shows, "Count")?.int()? > 0 {
            let window = item(&shows, 1)?;
            let view = get_obj(&window, "View")?;
            let pres = get_obj(&window, "Presentation")?;
            let count = get(&get_obj(&pres, "Slides")?, "Count")?.int()?;
            // View.Slide fails on the black/white/end screens; the position still works.
            let (index, notes) = match get_obj(&view, "Slide") {
                Ok(slide) => (
                    get(&slide, "SlideIndex")?.int()?,
                    slide_notes(&slide).unwrap_or_default(),
                ),
                Err(_) => (get(&view, "CurrentShowPosition")?.int()?, String::new()),
            };
            return Ok(Some(SlideInfo {
                index: index.max(0) as u32,
                count: count.max(0) as u32,
                notes,
                showing: true,
            }));
        }
        if get(&get_obj(&app, "Presentations")?, "Count")?.int()? == 0 {
            return Ok(None);
        }
        let pres = get_obj(&app, "ActivePresentation")?;
        let count = get(&get_obj(&pres, "Slides")?, "Count")?.int()?;
        let slide = get_obj(&app, "ActiveWindow")
            .and_then(|w| get_obj(&w, "View"))
            .and_then(|v| get_obj(&v, "Slide"));
        let (index, notes) = match slide {
            Ok(s) => (get(&s, "SlideIndex")?.int()?, slide_notes(&s).unwrap_or_default()),
            Err(_) => (0, String::new()),
        };
        Ok(Some(SlideInfo {
            index: index.max(0) as u32,
            count: count.max(0) as u32,
            notes,
            showing: false,
        }))
    })()
    .map_err(com_err)
}

// ---------------------------------------------------------------- laser overlay

#[derive(Clone, Copy, Debug)]
enum Cmd {
    Show { rect: Rect, x: f32, y: f32, trail: bool },
    Hide,
}

fn overlay_send(cmd: Cmd) -> PResult<()> {
    static TX: OnceLock<Mutex<Option<mpsc::Sender<Cmd>>>> = OnceLock::new();
    let slot = TX.get_or_init(|| Mutex::new(None));
    let mut tx = slot.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(t) = tx.as_ref()
        && t.send(cmd).is_ok()
    {
        return Ok(());
    }
    if matches!(cmd, Cmd::Hide) {
        *tx = None;
        return Ok(());
    }
    let (t, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("helm-laser".into())
        .spawn(move || overlay_thread(rx))
        .map_err(|e| RpcError::internal(format!("laser overlay: {e}")))?;
    let _ = t.send(cmd);
    *tx = Some(t);
    Ok(())
}

fn overlay_thread(rx: mpsc::Receiver<Cmd>) {
    let mut win: Option<(OverlayWindow, Laser)> = None;
    let mut visible = false;
    let mut last_input = Instant::now();
    loop {
        let wait = match &win {
            Some((_, l)) if visible && l.animating() => FRAME,
            _ if visible => Duration::from_millis(250),
            _ => Duration::from_secs(5),
        };
        let first = match rx.recv_timeout(wait) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut cmds: Vec<Cmd> = first.into_iter().collect();
        cmds.extend(rx.try_iter());
        let now = Instant::now();
        for cmd in cmds {
            match cmd {
                Cmd::Show { rect, x, y, trail } => {
                    last_input = now;
                    if win.as_ref().is_none_or(|(w, _)| w.rect != rect) {
                        win = None; // destroy the old window first
                        match OverlayWindow::create(rect) {
                            Ok(w) => win = Some((w, Laser::new(rect.width, rect.height))),
                            Err(e) => {
                                tracing::warn!("laser overlay unavailable: {e}");
                                continue;
                            }
                        }
                    }
                    if let Some((w, l)) = win.as_mut() {
                        l.push(x, y, trail, now);
                        if !visible {
                            w.show();
                            visible = true;
                        }
                    }
                }
                Cmd::Hide => {
                    if let Some((w, l)) = win.as_mut() {
                        if let Some(d) = l.clear() {
                            w.present(l, d);
                        }
                        w.hide();
                    }
                    visible = false;
                }
            }
        }
        pump_messages();
        if visible && now.duration_since(last_input) > IDLE_HIDE {
            if let Some((w, l)) = win.as_mut() {
                if let Some(d) = l.clear() {
                    w.present(l, d);
                }
                w.hide();
            }
            visible = false;
        }
        if visible
            && let Some((w, l)) = win.as_mut()
            && let Some(d) = l.render(now)
        {
            w.present(l, d);
        }
    }
}

fn pump_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

struct OverlayWindow {
    hwnd: HWND,
    rect: Rect,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
}

impl OverlayWindow {
    fn create(rect: Rect) -> windows::core::Result<OverlayWindow> {
        let (w, h) = (rect.width.max(1) as i32, rect.height.max(1) as i32);
        unsafe {
            let instance = GetModuleHandleW(None)?;
            static CLASS: OnceLock<u16> = OnceLock::new();
            let atom = *CLASS.get_or_init(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance.into(),
                    lpszClassName: w!("HelmLaserOverlay"),
                    ..Default::default()
                };
                RegisterClassW(&wc)
            });
            if atom == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("HelmLaserOverlay"),
                w!("Helm laser pointer"),
                WS_POPUP,
                rect.x,
                rect.y,
                w,
                h,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    let _ = DestroyWindow(hwnd);
                    return Err(e);
                }
            };
            let old = SelectObject(dc, bitmap.into());
            std::ptr::write_bytes(bits as *mut u32, 0, (w * h) as usize);
            let win = OverlayWindow {
                hwnd,
                rect,
                dc,
                bitmap,
                old,
                bits: bits as *mut u32,
            };
            win.update(None);
            Ok(win)
        }
    }

    /// Copy `dirty` from the canvas and push it to the layered window.
    fn present(&self, laser: &Laser, dirty: IRect) {
        let c = &laser.canvas;
        let d = dirty.clip(c.width, c.height);
        if d.is_empty() {
            return;
        }
        for y in d.y0..d.y1 {
            let row = y as usize * c.width as usize;
            let src = &c.pixels[row + d.x0 as usize..row + d.x1 as usize];
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr(), self.bits.add(row + d.x0 as usize), src.len());
            }
        }
        self.update(Some(RECT {
            left: d.x0,
            top: d.y0,
            right: d.x1,
            bottom: d.y1,
        }));
    }

    fn update(&self, dirty: Option<RECT>) {
        let dst = POINT {
            x: self.rect.x,
            y: self.rect.y,
        };
        let size = SIZE {
            cx: self.rect.width.max(1) as i32,
            cy: self.rect.height.max(1) as i32,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &dst,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: dirty.as_ref().map_or(std::ptr::null(), |r| r as *const RECT),
        };
        unsafe {
            if !UpdateLayeredWindowIndirect(self.hwnd, &info).as_bool() {
                tracing::debug!("UpdateLayeredWindowIndirect failed");
            }
        }
    }

    fn show(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
