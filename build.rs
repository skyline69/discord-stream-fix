use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=include/wels");
    let cflags = Command::new("pkg-config")
        .args(["--cflags", "libavcodec", "libavutil"])
        .output()
        .expect("pkg-config is required");
    assert!(
        cflags.status.success(),
        "FFmpeg development headers are required"
    );
    let version = Command::new("pkg-config")
        .args(["--modversion", "libavcodec"])
        .output()
        .expect("pkg-config is required");
    let version = String::from_utf8(version.stdout).expect("invalid FFmpeg version");
    assert!(
        version.trim().starts_with("62."),
        "libavcodec 62 development headers are required (found {})",
        version.trim()
    );
    let flags = String::from_utf8(cflags.stdout).expect("invalid pkg-config output");
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-Iinclude")
        .clang_args(flags.split_whitespace())
        .allowlist_type("AVCodec|AVCodecContext|AVPacket|AVFrame|AVDictionary|AVCodecID|ISVCDecoder.*|ISVCEncoder.*|SBufferInfo|SDecodingParam|DECODER_OPTION|DECODING_STATE")
        .allowlist_var("AV_CODEC_ID_H264|AV_PIX_FMT_YUV420P|AV_PIX_FMT_YUVJ420P|DECODER_OPTION_PROFILE|DECODER_OPTION_LEVEL|dsErrorFree|dsBitstreamError")
        .generate_comments(false)
        .layout_tests(false)
        .generate()
        .expect("could not generate FFmpeg/OpenH264 bindings");
    bindings
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings.rs"))
        .expect("could not write bindings");
}
