#![deny(unsafe_op_in_unsafe_fn)]

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::type_complexity
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use ffi::*;
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::os::unix::ffi::OsStrExt;
use std::ptr::{self, null, null_mut};
use std::sync::OnceLock;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlmopen(namespace: c_long, file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const LM_ID_NEWLM: c_long = -1;
const AVERROR_EOF: c_int = -541_478_725;
const AVERROR_EAGAIN: c_int = -11;

type CreateEncoder = unsafe extern "C" fn(*mut *mut ISVCEncoder) -> c_int;
type DestroyEncoder = unsafe extern "C" fn(*mut ISVCEncoder);

struct Api {
    find_decoder: unsafe extern "C" fn(AVCodecID) -> *const AVCodec,
    alloc_context: unsafe extern "C" fn(*const AVCodec) -> *mut AVCodecContext,
    open_codec:
        unsafe extern "C" fn(*mut AVCodecContext, *const AVCodec, *mut *mut AVDictionary) -> c_int,
    free_context: unsafe extern "C" fn(*mut *mut AVCodecContext),
    send_packet: unsafe extern "C" fn(*mut AVCodecContext, *const AVPacket) -> c_int,
    receive_frame: unsafe extern "C" fn(*mut AVCodecContext, *mut AVFrame) -> c_int,
    frame_alloc: unsafe extern "C" fn() -> *mut AVFrame,
    frame_free: unsafe extern "C" fn(*mut *mut AVFrame),
    frame_unref: unsafe extern "C" fn(*mut AVFrame),
    packet_alloc: unsafe extern "C" fn() -> *mut AVPacket,
    packet_free: unsafe extern "C" fn(*mut *mut AVPacket),
    create_encoder: Option<CreateEncoder>,
    destroy_encoder: Option<DestroyEncoder>,
}
static API: OnceLock<Option<Api>> = OnceLock::new();

fn api() -> Option<&'static Api> {
    API.get_or_init(|| {
        // FFmpeg must live in its own namespace: Discord embeds a different ABI.
        unsafe {
            let av = dlmopen(LM_ID_NEWLM, c"libavcodec.so.62".as_ptr(), RTLD_NOW);
            if av.is_null() {
                return None;
            }
            macro_rules! required {
                ($name:literal, $ty:ty) => {{
                    let address = dlsym(
                        av,
                        CStr::from_bytes_with_nul_unchecked(concat!($name, "\0").as_bytes())
                            .as_ptr(),
                    );
                    if address.is_null() {
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, $ty>(address)
                }};
            }
            let original_path = std::env::var_os("DISCORD_OPENH264_ORIGINAL").or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    std::path::PathBuf::from(home)
                        .join(".local/share/discord-stream-fix/openh264-original.so")
                        .into_os_string()
                })
            });
            let original = original_path
                .and_then(|p| CString::new(p.as_os_str().as_bytes()).ok())
                .map_or(null_mut(), |p| dlopen(p.as_ptr(), RTLD_NOW));
            let enc = if original.is_null() {
                null_mut()
            } else {
                dlsym(original, c"WelsCreateSVCEncoder".as_ptr())
            };
            let destroy = if original.is_null() {
                null_mut()
            } else {
                dlsym(original, c"WelsDestroySVCEncoder".as_ptr())
            };
            Some(Api {
                find_decoder: required!(
                    "avcodec_find_decoder",
                    unsafe extern "C" fn(AVCodecID) -> *const AVCodec
                ),
                alloc_context: required!(
                    "avcodec_alloc_context3",
                    unsafe extern "C" fn(*const AVCodec) -> *mut AVCodecContext
                ),
                open_codec: required!(
                    "avcodec_open2",
                    unsafe extern "C" fn(
                        *mut AVCodecContext,
                        *const AVCodec,
                        *mut *mut AVDictionary,
                    ) -> c_int
                ),
                free_context: required!(
                    "avcodec_free_context",
                    unsafe extern "C" fn(*mut *mut AVCodecContext)
                ),
                send_packet: required!(
                    "avcodec_send_packet",
                    unsafe extern "C" fn(*mut AVCodecContext, *const AVPacket) -> c_int
                ),
                receive_frame: required!(
                    "avcodec_receive_frame",
                    unsafe extern "C" fn(*mut AVCodecContext, *mut AVFrame) -> c_int
                ),
                frame_alloc: required!("av_frame_alloc", unsafe extern "C" fn() -> *mut AVFrame),
                frame_free: required!("av_frame_free", unsafe extern "C" fn(*mut *mut AVFrame)),
                frame_unref: required!("av_frame_unref", unsafe extern "C" fn(*mut AVFrame)),
                packet_alloc: required!("av_packet_alloc", unsafe extern "C" fn() -> *mut AVPacket),
                packet_free: required!("av_packet_free", unsafe extern "C" fn(*mut *mut AVPacket)),
                create_encoder: if enc.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<*mut c_void, CreateEncoder>(enc))
                },
                destroy_encoder: if destroy.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<*mut c_void, DestroyEncoder>(destroy))
                },
            })
        }
    })
    .as_ref()
}

#[repr(C)]
struct Decoder {
    vtable: ISVCDecoder,
    context: *mut AVCodecContext,
    frame: *mut AVFrame,
}

unsafe extern "C" fn initialize(this: *mut ISVCDecoder, _param: *const SDecodingParam) -> c_long {
    // OpenH264 owns the decoder pointer and calls methods using this vtable.
    unsafe {
        let d = &mut *this.cast::<Decoder>();
        let Some(a) = api() else { return 1 };
        let codec = (a.find_decoder)(AVCodecID_AV_CODEC_ID_H264);
        if codec.is_null() {
            return 1;
        }
        d.context = (a.alloc_context)(codec);
        if d.context.is_null() || (a.open_codec)(d.context, codec, null_mut()) < 0 {
            return 1;
        }
        d.frame = (a.frame_alloc)();
        c_long::from(d.frame.is_null())
    }
}
unsafe extern "C" fn uninitialize(this: *mut ISVCDecoder) -> c_long {
    unsafe {
        let d = &mut *this.cast::<Decoder>();
        if let Some(a) = api() {
            if !d.frame.is_null() {
                (a.frame_free)(&mut d.frame);
            }
            if !d.context.is_null() {
                (a.free_context)(&mut d.context);
            }
        }
    }
    0
}
unsafe extern "C" fn decode(
    this: *mut ISVCDecoder,
    data: *const u8,
    len: c_int,
    dst: *mut *mut u8,
    info: *mut SBufferInfo,
) -> DECODING_STATE {
    unsafe {
        let d = &mut *this.cast::<Decoder>();
        let Some(a) = api() else {
            return DECODING_STATE_dsBitstreamError;
        };
        if d.context.is_null() || d.frame.is_null() || dst.is_null() || info.is_null() || len < 0 {
            return DECODING_STATE_dsBitstreamError;
        }
        let mut packet = (a.packet_alloc)();
        if packet.is_null() {
            return DECODING_STATE_dsBitstreamError;
        }
        (*packet).data = data.cast_mut();
        (*packet).size = len;
        (*packet).pts = (*info).uiInBsTimeStamp as i64;
        (*packet).dts = i64::MIN;
        let result = (a.send_packet)(d.context, if data.is_null() { null() } else { packet });
        // The packet borrows the caller's bytes: prevent av_packet_free from owning them.
        (*packet).data = null_mut();
        (*packet).size = 0;
        (a.packet_free)(&mut packet);
        if result < 0 && result != AVERROR_EOF {
            return DECODING_STATE_dsBitstreamError;
        }
        (a.frame_unref)(d.frame);
        let result = (a.receive_frame)(d.context, d.frame);
        if result == AVERROR_EAGAIN || result == AVERROR_EOF {
            (*info).iBufferStatus = 0;
            return DECODING_STATE_dsErrorFree;
        }
        let frame = &*d.frame;
        if result < 0
            || (frame.format != AVPixelFormat_AV_PIX_FMT_YUV420P
                && frame.format != AVPixelFormat_AV_PIX_FMT_YUVJ420P)
            || frame.linesize[1] != frame.linesize[2]
        {
            return DECODING_STATE_dsBitstreamError;
        }
        ptr::copy_nonoverlapping(frame.data.as_ptr(), dst, 3);
        (*info).iBufferStatus = 1;
        (*info).uiOutYuvTimeStamp = frame.pts as u64;
        let system = &mut (*info).UsrData.sSystemBuffer;
        system.iWidth = frame.width;
        system.iHeight = frame.height;
        system.iStride = [frame.linesize[0], frame.linesize[1]];
        DECODING_STATE_dsErrorFree
    }
}
unsafe extern "C" fn flush(
    this: *mut ISVCDecoder,
    dst: *mut *mut u8,
    info: *mut SBufferInfo,
) -> DECODING_STATE {
    unsafe { decode(this, null(), 0, dst, info) }
}
unsafe extern "C" fn set_option(_: *mut ISVCDecoder, _: DECODER_OPTION, _: *mut c_void) -> c_long {
    0
}
unsafe extern "C" fn get_option(
    _: *mut ISVCDecoder,
    option: DECODER_OPTION,
    value: *mut c_void,
) -> c_long {
    if !value.is_null()
        && (option == DECODER_OPTION_DECODER_OPTION_PROFILE
            || option == DECODER_OPTION_DECODER_OPTION_LEVEL)
    {
        unsafe {
            *value.cast::<c_int>() = 0;
        }
    }
    0
}
static VTABLE: ISVCDecoderVtbl = ISVCDecoderVtbl {
    Initialize: Some(initialize),
    Uninitialize: Some(uninitialize),
    DecodeFrame: None,
    DecodeFrameNoDelay: Some(decode),
    DecodeFrame2: Some(decode),
    FlushFrame: Some(flush),
    DecodeParser: None,
    DecodeFrameEx: None,
    SetOption: Some(set_option),
    GetOption: Some(get_option),
};

/// # Safety
/// `out` must be a valid writable OpenH264 decoder pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn WelsCreateDecoder(out: *mut *mut ISVCDecoder) -> c_long {
    if out.is_null() || api().is_none() {
        return 1;
    }
    let d = Box::new(Decoder {
        vtable: &VTABLE,
        context: null_mut(),
        frame: null_mut(),
    });
    unsafe {
        *out = Box::into_raw(d).cast::<ISVCDecoder>();
    }
    0
}
/// # Safety
/// `decoder` must be null or a pointer returned by `WelsCreateDecoder`, not previously destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn WelsDestroyDecoder(decoder: *mut ISVCDecoder) {
    if !decoder.is_null() {
        unsafe {
            uninitialize(decoder);
            drop(Box::from_raw(decoder.cast::<Decoder>()));
        }
    }
}
/// # Safety
/// `out` must satisfy the original OpenH264 encoder API contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn WelsCreateSVCEncoder(out: *mut *mut ISVCEncoder) -> c_int {
    match api().and_then(|a| a.create_encoder) {
        Some(create) => unsafe { create(out) },
        None => 1,
    }
}
/// # Safety
/// `encoder` must satisfy the original OpenH264 encoder API contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn WelsDestroySVCEncoder(encoder: *mut ISVCEncoder) {
    if let Some(destroy) = api().and_then(|a| a.destroy_encoder) {
        unsafe {
            destroy(encoder);
        }
    }
}
