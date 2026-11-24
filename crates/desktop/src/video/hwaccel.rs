//! FFmpeg hardware decoding: VAAPI on Linux, D3D11VA on Windows.
//!
//! `ffmpeg-next` has no safe API for hwaccel, so this module calls
//! `ffmpeg-sys-next` directly. It is the only `unsafe` code in the desktop
//! crate. Decoded frames live in GPU memory; [`download`] copies them to an
//! NV12 frame in system memory for the wgpu renderer.

use std::fmt;
use std::ptr;

use ffmpeg::ffi;
use ffmpeg_next as ffmpeg;

/// Which decoder produced the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Software,
    Vaapi,
    D3d11va,
}

impl Backend {
    /// The hardware backend for this OS, if any.
    pub fn platform_default() -> Option<Self> {
        if cfg!(windows) {
            Some(Self::D3d11va)
        } else if cfg!(target_os = "linux") {
            Some(Self::Vaapi)
        } else {
            None
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Vaapi,
            2 => Self::D3d11va,
            _ => Self::Software,
        }
    }

    fn device_type(self) -> Option<ffi::AVHWDeviceType> {
        match self {
            Self::Software => None,
            Self::Vaapi => Some(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI),
            Self::D3d11va => Some(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA),
        }
    }

    /// Pixel format of frames that stay on the GPU.
    pub fn hw_pixel_format(self) -> Option<ffi::AVPixelFormat> {
        match self {
            Self::Software => None,
            Self::Vaapi => Some(ffi::AVPixelFormat::AV_PIX_FMT_VAAPI),
            Self::D3d11va => Some(ffi::AVPixelFormat::AV_PIX_FMT_D3D11),
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Software => "Software",
            Self::Vaapi => "VAAPI",
            Self::D3d11va => "D3D11VA",
        })
    }
}

/// Whether a device for `backend` can be opened on this system.
pub fn probe(backend: Backend) -> bool {
    let Some(device_type) = backend.device_type() else {
        return false;
    };
    let mut device: *mut ffi::AVBufferRef = ptr::null_mut();
    // SAFETY: as in `attach`; the device is released right away.
    unsafe {
        let ret =
            ffi::av_hwdevice_ctx_create(&mut device, device_type, ptr::null(), ptr::null_mut(), 0);
        if !device.is_null() {
            ffi::av_buffer_unref(&mut device);
        }
        ret >= 0
    }
}

/// Sets up hardware decoding on a decoder context that hasn't been opened
/// yet. Returns an error message if the decoder or the system can't do it;
/// the context is then left untouched (software decoding).
pub fn attach(
    ctx: &mut ffmpeg::codec::Context,
    codec: &ffmpeg::Codec,
    backend: Backend,
) -> Result<(), String> {
    let (Some(device_type), Some(pix_fmt)) = (backend.device_type(), backend.hw_pixel_format())
    else {
        return Err("no hardware backend".into());
    };
    if !codec_supports(codec, device_type, pix_fmt) {
        return Err(format!(
            "FFmpeg's {} decoder has no {backend} support",
            codec.name()
        ));
    }

    let mut device: *mut ffi::AVBufferRef = ptr::null_mut();
    // SAFETY: `device` is a valid out pointer; null device name and options
    // pick the default adapter.
    let ret = unsafe {
        ffi::av_hwdevice_ctx_create(&mut device, device_type, ptr::null(), ptr::null_mut(), 0)
    };
    if ret < 0 || device.is_null() {
        return Err(format!(
            "could not open a {backend} device: {}",
            ffmpeg::Error::from(ret)
        ));
    }
    // SAFETY: the context is valid and not opened yet. It takes its own
    // reference to the device; ours is released right after. `opaque` holds
    // the hardware pixel format for `get_format` (a plain integer, no
    // allocation to free).
    unsafe {
        let raw = ctx.as_mut_ptr();
        (*raw).hw_device_ctx = ffi::av_buffer_ref(device);
        (*raw).opaque = pix_fmt as i32 as isize as *mut std::ffi::c_void;
        (*raw).get_format = Some(get_format);
        ffi::av_buffer_unref(&mut device);
    }
    Ok(())
}

/// Whether `codec` can decode into `pix_fmt` frames with a `device_type` device.
fn codec_supports(
    codec: &ffmpeg::Codec,
    device_type: ffi::AVHWDeviceType,
    pix_fmt: ffi::AVPixelFormat,
) -> bool {
    for i in 0.. {
        // SAFETY: valid codec; returns null past the last config.
        let config = unsafe { ffi::avcodec_get_hw_config(codec.as_ptr(), i) };
        if config.is_null() {
            return false;
        }
        // SAFETY: non-null configs point to static data.
        let config = unsafe { &*config };
        if config.device_type == device_type
            && config.pix_fmt == pix_fmt
            && config.methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
        {
            return true;
        }
    }
    false
}

/// Picks the hardware format if the decoder offers it, else the first
/// software format (FFmpeg then decodes in software).
unsafe extern "C" fn get_format(
    ctx: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: FFmpeg passes its context and a list ending in AV_PIX_FMT_NONE.
    unsafe {
        let wanted = (*ctx).opaque as isize as i32;
        let mut first_software = ffi::AVPixelFormat::AV_PIX_FMT_NONE;
        let mut p = formats;
        while *p != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p as i32 == wanted {
                return *p;
            }
            let desc = ffi::av_pix_fmt_desc_get(*p);
            let is_hw = !desc.is_null() && (*desc).flags & ffi::AV_PIX_FMT_FLAG_HWACCEL as u64 != 0;
            if !is_hw && first_software == ffi::AVPixelFormat::AV_PIX_FMT_NONE {
                first_software = *p;
            }
            p = p.add(1);
        }
        tracing::warn!("hardware decoding not available for this stream; using software");
        first_software
    }
}

/// Whether a decoded frame is in GPU memory.
pub fn is_hw_frame(frame: &ffmpeg::frame::Video) -> bool {
    // SAFETY: valid frame.
    unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() }
}

/// Copies a GPU frame into `dst` in system memory (usually NV12).
pub fn download(
    src: &ffmpeg::frame::Video,
    dst: &mut ffmpeg::frame::Video,
) -> Result<(), ffmpeg::Error> {
    // SAFETY: both frames are valid; `dst` is reset so FFmpeg allocates its
    // buffers in the frames context's software format.
    unsafe {
        ffi::av_frame_unref(dst.as_mut_ptr());
        let ret = ffi::av_hwframe_transfer_data(dst.as_mut_ptr(), src.as_ptr(), 0);
        if ret < 0 {
            return Err(ffmpeg::Error::from(ret));
        }
        // Colour info isn't copied by the transfer.
        let (d, s) = (dst.as_mut_ptr(), src.as_ptr());
        (*d).colorspace = (*s).colorspace;
        (*d).color_range = (*s).color_range;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_round_trip() {
        for b in [Backend::Software, Backend::Vaapi, Backend::D3d11va] {
            assert_eq!(Backend::from_u8(b.as_u8()), b);
        }
        assert_eq!(Backend::D3d11va.to_string(), "D3D11VA");
    }
}
