//! The shared Direct3D 11 device used by capture, colour conversion and the encoder.

use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::{HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{DXGI_ADAPTER_DESC, IDXGIDevice};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::core::Interface;

use crate::error::{Context, Error, Result};

/// A hardware D3D11 device created with BGRA (needed by Windows.Graphics.Capture) and VIDEO
/// (needed by the video processor and hardware encoders) support, multithread protected because
/// capture callbacks, our pipeline thread and the encoder MFT all use its immediate context.
#[derive(Clone)]
pub struct D3dDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// The same device wrapped for WinRT (`Direct3D11CaptureFramePool` wants this).
    pub winrt: IDirect3DDevice,
    pub adapter_luid: LUID,
    pub adapter_name: String,
}

impl D3dDevice {
    pub fn new() -> Result<Self> {
        let levels: [D3D_FEATURE_LEVEL; 4] =
            [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        // SAFETY: out-pointers to locals; default adapter, no software module.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .ctx("D3D11CreateDevice")?;
        }
        let device = device.ok_or_else(|| Error::Unsupported("D3D11CreateDevice returned no device".into()))?;
        let context = context.ok_or_else(|| Error::Unsupported("D3D11CreateDevice returned no context".into()))?;

        // SAFETY: COM calls on valid interfaces.
        unsafe {
            if let Ok(mt) = context.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
        }

        let dxgi: IDXGIDevice = device.cast().ctx("QueryInterface(IDXGIDevice)")?;
        // SAFETY: COM calls on valid interfaces.
        let (adapter_luid, adapter_name) = unsafe {
            let adapter = dxgi.GetAdapter().ctx("IDXGIDevice::GetAdapter")?;
            let desc: DXGI_ADAPTER_DESC = adapter.GetDesc().ctx("IDXGIAdapter::GetDesc")?;
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            (desc.AdapterLuid, String::from_utf16_lossy(&desc.Description[..end]))
        };
        // SAFETY: documented interop entry point; takes a valid IDXGIDevice.
        let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .ctx("CreateDirect3D11DeviceFromDXGIDevice")?
            .cast()
            .ctx("QueryInterface(IDirect3DDevice)")?;
        Ok(Self { device, context, winrt, adapter_luid, adapter_name })
    }
}
