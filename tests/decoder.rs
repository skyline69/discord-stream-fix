use std::{env, fs, io::Write, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let status = command.status().expect("failed to start test command");
    assert!(status.success(), "test command failed: {command:?}");
}

#[test]
fn discord_wrapper_decodes_like_native_ffmpeg() {
    let home = PathBuf::from(env::var_os("HOME").expect("HOME missing"));
    let discord = home.join(".config/discord/app-1.0.167/libffmpeg.so");
    if !discord.is_file() || Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("Skipping decoder integration: Discord 1.0.167 and FFmpeg are required");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    run(Command::new("cargo")
        .args(["build", "--locked", "--lib"])
        .current_dir(&root));
    let shim = root.join("target/debug/libdiscord_stream_fix.so");
    let runner = env!("CARGO_BIN_EXE_decoder_check");
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("input.h264");
    let packets = dir.path().join("packets");
    let output = dir.path().join("output.yuv");
    let reference = dir.path().join("reference.yuv");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30",
            "-t",
            "0.5",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-x264-params",
            "aud=1",
            "-f",
            "h264",
        ])
        .arg(&video));
    let bytes = fs::read(&video).unwrap();
    let marker = [0, 0, 0, 1, 9];
    let positions: Vec<_> = bytes
        .windows(marker.len())
        .enumerate()
        .filter_map(|(i, value)| (value == marker).then_some(i))
        .collect();
    assert!(!positions.is_empty(), "no H.264 access units found");
    let mut file = fs::File::create(&packets).unwrap();
    for (index, start) in positions.iter().enumerate() {
        let end = positions.get(index + 1).copied().unwrap_or(bytes.len());
        let packet = &bytes[*start..end];
        file.write_all(&(packet.len() as u32).to_le_bytes())
            .unwrap();
        file.write_all(packet).unwrap();
    }
    drop(file);
    run(Command::new(runner)
        .arg(&discord)
        .arg(&shim)
        .arg(&packets)
        .arg(&output));
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&video)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
        .arg(&reference));
    let decoded = fs::read(output).unwrap();
    assert!(!decoded.is_empty());
    assert_eq!(decoded, fs::read(reference).unwrap());
}

#[test]
fn forwards_encoder_to_original_openh264() {
    let home = PathBuf::from(env::var_os("HOME").expect("HOME missing"));
    let original = home.join(".cache/discord-stream-fix/libopenh264-2.5.1-linux64.7.so");
    if !original.is_file() {
        eprintln!("Skipping encoder test: Cisco OpenH264 2.5.1 is not cached");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    run(Command::new("cargo")
        .args(["build", "--locked", "--lib"])
        .current_dir(&root));
    let temporary_home = tempfile::tempdir().unwrap();
    let destination = temporary_home
        .path()
        .join(".local/share/discord-stream-fix/openh264-original.so");
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::copy(original, destination).unwrap();
    run(Command::new(env!("CARGO_BIN_EXE_decoder_check"))
        .arg("--encoder")
        .arg(root.join("target/debug/libdiscord_stream_fix.so"))
        .env("HOME", temporary_home.path())
        .env_remove("DISCORD_OPENH264_ORIGINAL"));
}
