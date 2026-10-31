//! GPU colour conversion + scaling: BGRA capture texture → NV12 encoder texture with the
//! D3D11 video processor (`ID3D11VideoProcessor`). The image is scaled to the output size and
//! letterboxed if the source aspect ratio changes (e.g. a captured window is resized).

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VIDEO_ENCODER,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_COLOR,
    D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::config::letterbox_rect;
use crate::d3d::D3dDevice;
use crate::error::{Context, Error, Result};

/// BGRA → NV12 converter bound to one output size.
pub struct Nv12Converter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    out_w: u32,
    out_h: u32,
    /// Our own BGRA copy target, used only if the capture texture cannot be bound as a
    /// video-processor input directly.
    intermediate: Option<(ID3D11Texture2D, u32, u32)>,
    use_intermediate: bool,
    last_rects: Option<(RECT, RECT)>,
}

// SAFETY: every member is a free-threaded D3D11 interface on a multithread-protected device.
// The converter is used behind a Mutex, so calls are never concurrent.
unsafe impl Send for Nv12Converter {}

fn rect(l: u32, t: u32, r: u32, b: u32) -> RECT {
    RECT { left: l as i32, top: t as i32, right: r as i32, bottom: b as i32 }
}

fn rect_eq(a: &RECT, b: &RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

impl Nv12Converter {
    /// `in_w × in_h`: expected source size (a hint; other sizes still work).
    pub fn new(d3d: &D3dDevice, in_w: u32, in_h: u32, out_w: u32, out_h: u32, fps: u32) -> Result<Self> {
        let video_device: ID3D11VideoDevice = d3d.device.cast().ctx("QueryInterface(ID3D11VideoDevice)")?;
        let video_context: ID3D11VideoContext = d3d.context.cast().ctx("QueryInterface(ID3D11VideoContext)")?;
        let rate = DXGI_RATIONAL { Numerator: fps, Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: in_w.max(2),
            InputHeight: in_h.max(2),
            OutputFrameRate: rate,
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        // SAFETY: COM calls with valid descriptors and interfaces.
        unsafe {
            let enumerator =
                video_device.CreateVideoProcessorEnumerator(&desc).ctx("CreateVideoProcessorEnumerator")?;
            let check = |f: DXGI_FORMAT, bit: i32| -> bool {
                enumerator.CheckVideoProcessorFormat(f).map(|s| s & bit as u32 != 0).unwrap_or(false)
            };
            if !check(DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0) {
                return Err(Error::Unsupported("video processor cannot read BGRA".into()));
            }
            if !check(DXGI_FORMAT_NV12, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0) {
                return Err(Error::Unsupported("video processor cannot write NV12".into()));
            }
            let processor = video_device.CreateVideoProcessor(&enumerator, 0).ctx("CreateVideoProcessor")?;

            video_context.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            // No denoise / edge enhancement etc.: they cost time and blur text.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            // Colour: full-range sRGB in, BT.709 limited-range YCbCr out (signalled in the SPS).
            if let Ok(vc1) = video_context.cast::<ID3D11VideoContext1>() {
                vc1.VideoProcessorSetStreamColorSpace1(&processor, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                vc1.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            } else {
                // Bitfield: Usage:1 RGB_Range:1 YCbCr_Matrix:1 YCbCr_xvYCC:1 Nominal_Range:2.
                let input = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 }; // full-range RGB
                let output = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) }; // 709, 16-235
                video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &input);
                video_context.VideoProcessorSetOutputColorSpace(&processor, &output);
            }
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } },
            };
            video_context.VideoProcessorSetOutputBackgroundColor(&processor, false, &black);
            let full = rect(0, 0, out_w, out_h);
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full));

            Ok(Self {
                device: d3d.device.clone(),
                context: d3d.context.clone(),
                video_device,
                video_context,
                enumerator,
                processor,
                out_w,
                out_h,
                intermediate: None,
                use_intermediate: false,
                last_rects: None,
            })
        }
    }

    pub fn output_size(&self) -> (u32, u32) {
        (self.out_w, self.out_h)
    }

    /// Create an NV12 texture the converter can write and an encoder can read.
    pub fn create_output_texture(&self) -> Result<ID3D11Texture2D> {
        let attempts: [D3D11_BIND_FLAG; 3] = [
            D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0),
            D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0),
            D3D11_BIND_RENDER_TARGET,
        ];
        let mut last_err = None;
        for bind in attempts {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.out_w,
                Height: self.out_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            // SAFETY: valid descriptor, out-pointer to a local.
            match unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) } {
                Ok(()) => {
                    if let Some(t) = tex {
                        return Ok(t);
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(Error::Windows {
            context: "CreateTexture2D(NV12)",
            source: last_err.unwrap_or_else(windows::core::Error::empty),
        })
    }

    /// Output view for one of the textures made by [`Self::create_output_texture`].
    pub fn output_view(&self, tex: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorOutputView> {
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut view = None;
        // SAFETY: texture created on the same device; out-pointer to a local.
        unsafe { self.video_device.CreateVideoProcessorOutputView(tex, &self.enumerator, &desc, Some(&mut view)) }
            .ctx("CreateVideoProcessorOutputView")?;
        view.ok_or_else(|| Error::Unsupported("no output view".into()))
    }

    fn input_view(&self, tex: &ID3D11Texture2D) -> windows::core::Result<ID3D11VideoProcessorInputView> {
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
            },
        };
        let mut view = None;
        // SAFETY: as above.
        unsafe { self.video_device.CreateVideoProcessorInputView(tex, &self.enumerator, &desc, Some(&mut view))? };
        view.ok_or_else(windows::core::Error::empty)
    }

    /// Copy `src` into our own BGRA texture (fallback path) and return it.
    fn copy_to_intermediate(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        let mut sd = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid texture.
        unsafe { src.GetDesc(&mut sd) };
        let reuse = matches!(&self.intermediate, Some((_, w, h)) if *w == sd.Width && *h == sd.Height);
        if !reuse {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: sd.Width,
                Height: sd.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            // SAFETY: valid descriptor.
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }.ctx("CreateTexture2D(BGRA)")?;
            let tex = tex.ok_or_else(|| Error::Unsupported("no texture".into()))?;
            self.intermediate = Some((tex, sd.Width, sd.Height));
        }
        let (tex, _, _) = self.intermediate.as_ref().expect("set above");
        // SAFETY: same size and format, both on our device.
        unsafe { self.context.CopyResource(tex, src) };
        Ok(tex.clone())
    }

    /// Convert the `content_w × content_h` top-left region of `src` into `out`.
    pub fn convert(
        &mut self,
        src: &ID3D11Texture2D,
        content_w: u32,
        content_h: u32,
        out: &ID3D11VideoProcessorOutputView,
    ) -> Result<()> {
        let mut sd = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid texture.
        unsafe { src.GetDesc(&mut sd) };
        let cw = content_w.clamp(1, sd.Width.max(1));
        let ch = content_h.clamp(1, sd.Height.max(1));

        let input = if self.use_intermediate {
            let t = self.copy_to_intermediate(src)?;
            self.input_view(&t).ctx("CreateVideoProcessorInputView(copy)")?
        } else {
            match self.input_view(src) {
                Ok(v) => v,
                Err(_) => {
                    self.use_intermediate = true;
                    let t = self.copy_to_intermediate(src)?;
                    self.input_view(&t).ctx("CreateVideoProcessorInputView(copy)")?
                }
            }
        };

        let src_rect = rect(0, 0, cw, ch);
        let (l, t, r, b) = letterbox_rect(cw, ch, self.out_w, self.out_h);
        let dst_rect = rect(l, t, r, b);
        // SAFETY: COM calls on valid interfaces; the stream struct's view is released below.
        unsafe {
            let changed = !matches!(&self.last_rects, Some((s, d)) if rect_eq(s, &src_rect) && rect_eq(d, &dst_rect));
            if changed {
                self.video_context.VideoProcessorSetStreamSourceRect(&self.processor, 0, true, Some(&src_rect));
                self.video_context.VideoProcessorSetStreamDestRect(&self.processor, 0, true, Some(&dst_rect));
                self.last_rects = Some((src_rect, dst_rect));
            }
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(input)),
                ..Default::default()
            };
            let r = self.video_context.VideoProcessorBlt(&self.processor, out, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            r.ctx("VideoProcessorBlt")?;
        }
        Ok(())
    }

    /// Submit queued GPU work now (so the encoder sees finished frames sooner).
    pub fn flush(&self) {
        // SAFETY: valid context.
        unsafe { self.context.Flush() };
    }
}
