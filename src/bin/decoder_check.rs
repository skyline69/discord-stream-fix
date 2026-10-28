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
use std::{
    env,
    ffi::{CString, c_char, c_int, c_void},
    fs::File,
    io::{self, Read, Write},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    process::ExitCode,
    ptr,
};
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 2 && args[0] == "--encoder" {
        let path = CString::new(args[1].as_os_str().as_bytes())?;
        // The helper is a short-lived isolated process. The shim forwards these calls.
        unsafe {
            let handle = dlopen(path.as_ptr(), 2);
            if handle.is_null() {
                return Err("cannot load shim".into());
            }
            let create_ptr = dlsym(handle, c"WelsCreateSVCEncoder".as_ptr());
            let destroy_ptr = dlsym(handle, c"WelsDestroySVCEncoder".as_ptr());
            if create_ptr.is_null() || destroy_ptr.is_null() {
                return Err("missing encoder exports".into());
            }
            let create = std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*mut *mut ISVCEncoder) -> c_int,
            >(create_ptr);
            let destroy = std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut ISVCEncoder)>(
                destroy_ptr,
            );
            let mut encoder = ptr::null_mut();
            if create(&mut encoder) != 0 || encoder.is_null() {
                return Err("encoder creation failed".into());
            }
            destroy(encoder);
        }
        return Ok(());
    }
    if args.len() != 4 {
        return Err("usage: decoder_check DISCORD_LIBFFMPEG SHIM PACKETS OUTPUT".into());
    }
    let name = CString::new(args[0].as_os_str().as_bytes())?;
    // The test runner is isolated from Discord; the handle stays open for its lifetime.
    unsafe {
        let handle = dlopen(name.as_ptr(), 2);
        if handle.is_null() {
            return Err("cannot load Discord libffmpeg".into());
        }
        macro_rules! sym {
            ($name:literal,$ty:ty) => {{
                let p = dlsym(handle, concat!($name, "\0").as_ptr().cast());
                if p.is_null() {
                    return Err(concat!("missing symbol: ", $name).into());
                }
                std::mem::transmute::<*mut c_void, $ty>(p)
            }};
        }
        let set_path = sym!(
            "ff_libopenh264_set_library_path",
            unsafe extern "C" fn(*const c_char)
        );
        let find = sym!(
            "avcodec_find_decoder_by_name",
            unsafe extern "C" fn(*const c_char) -> *const AVCodec
        );
        let alloc = sym!(
            "avcodec_alloc_context3",
            unsafe extern "C" fn(*const AVCodec) -> *mut AVCodecContext
        );
        let open = sym!(
            "avcodec_open2",
            unsafe extern "C" fn(
                *mut AVCodecContext,
                *const AVCodec,
                *mut *mut AVDictionary,
            ) -> c_int
        );
        let send = sym!(
            "avcodec_send_packet",
            unsafe extern "C" fn(*mut AVCodecContext, *const AVPacket) -> c_int
        );
        let receive = sym!(
            "avcodec_receive_frame",
            unsafe extern "C" fn(*mut AVCodecContext, *mut AVFrame) -> c_int
        );
        let frame_alloc = sym!("av_frame_alloc", unsafe extern "C" fn() -> *mut AVFrame);
        let frame_unref = sym!("av_frame_unref", unsafe extern "C" fn(*mut AVFrame));
        let frame_free = sym!("av_frame_free", unsafe extern "C" fn(*mut *mut AVFrame));
        let packet_alloc = sym!("av_packet_alloc", unsafe extern "C" fn() -> *mut AVPacket);
        let packet_free = sym!("av_packet_free", unsafe extern "C" fn(*mut *mut AVPacket));
        let free = sym!(
            "avcodec_free_context",
            unsafe extern "C" fn(*mut *mut AVCodecContext)
        );
        let path = CString::new(args[1].as_os_str().as_bytes())?;
        set_path(path.as_ptr());
        let codec = find(c"libopenh264".as_ptr());
        if codec.is_null() {
            return Err("missing decoder".into());
        }
        let mut ctx = alloc(codec);
        let mut frame = frame_alloc();
        let mut pkt = packet_alloc();
        if ctx.is_null()
            || frame.is_null()
            || pkt.is_null()
            || open(ctx, codec, ptr::null_mut()) < 0
        {
            return Err("decoder initialization failed".into());
        }
        let mut input = File::open(PathBuf::from(&args[2]))?;
        let mut output = File::create(PathBuf::from(&args[3]))?;
        let mut header = [0u8; 4];
        while match input.read_exact(&mut header) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => false,
            Err(e) => return Err(e.into()),
        } {
            let size = u32::from_le_bytes(header) as usize;
            if size > 16 * 1024 * 1024 {
                return Err("oversized packet".into());
            }
            let mut bytes = vec![0u8; size];
            input.read_exact(&mut bytes)?;
            (*pkt).data = bytes.as_mut_ptr();
            (*pkt).size = size as c_int;
            if send(ctx, pkt) < 0 {
                return Err("send packet failed".into());
            }
            (*pkt).data = ptr::null_mut();
            (*pkt).size = 0;
            while receive(ctx, frame) == 0 {
                for plane in 0..3 {
                    let width = if plane == 0 {
                        (*frame).width
                    } else {
                        ((*frame).width + 1) / 2
                    };
                    let height = if plane == 0 {
                        (*frame).height
                    } else {
                        ((*frame).height + 1) / 2
                    };
                    for row in 0..height {
                        let p =
                            (*frame).data[plane].offset((row * (*frame).linesize[plane]) as isize);
                        output.write_all(std::slice::from_raw_parts(p, width as usize))?;
                    }
                }
                frame_unref(frame);
            }
        }
        send(ctx, ptr::null());
        while receive(ctx, frame) == 0 {
            for plane in 0..3 {
                let width = if plane == 0 {
                    (*frame).width
                } else {
                    ((*frame).width + 1) / 2
                };
                let height = if plane == 0 {
                    (*frame).height
                } else {
                    ((*frame).height + 1) / 2
                };
                for row in 0..height {
                    let p = (*frame).data[plane].offset((row * (*frame).linesize[plane]) as isize);
                    output.write_all(std::slice::from_raw_parts(p, width as usize))?;
                }
            }
            frame_unref(frame);
        }
        packet_free(&mut pkt);
        frame_free(&mut frame);
        free(&mut ctx);
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
