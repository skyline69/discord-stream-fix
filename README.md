# Discord stream fix

A per-user Linux workaround for Discord streams that fail with error 2012 or show corrupted H.264 video. It installs once; after installation, open Discord normally.

Discord's desktop client was observed requesting OpenH264 2.5.1 while its cleanup removed that file. Restoring it allowed streams to start, but both OpenH264 2.5.1 and 2.7.0 corrupted the affected H.264 stream. The Rust shim presents the OpenH264 interface Discord expects and decodes H.264 using Fedora's FFmpeg library in a separate linker namespace. Encoder calls still go to Cisco's OpenH264 library.

The Rust shim was tested with a live Discord stream on Fedora 44, KDE Wayland, and an NVIDIA GPU. Camera and outgoing streams have been tested end to end and they work! This workaround depends on Discord internals and may stop working after an update.

## Demo

### Before (artifacts visible or stream is loading)
<img width="1283" height="747" alt="before" src="https://github.com/user-attachments/assets/cce2109e-9f11-4779-a5a3-0b27e91588ed" />


### After (artifacts gone, stream loads)
<img width="1280" height="721" alt="after" src="https://github.com/user-attachments/assets/cf94431a-ef0c-4efe-9f60-7b9e3b1e8844" />


## Requirements

- Linux x86-64 with glibc and a running systemd user manager; tested on Fedora 44.
- Discord 1.0.167 installed at its usual per-user location.
- Rust 1.88 or newer, Cargo, Clang and libclang, pkg-config, and FFmpeg development headers for libavcodec 62.
- FFmpeg `libavcodec.so.62` at runtime.

The build generates Rust bindings from the system FFmpeg headers and included OpenH264 headers. FFmpeg and Discord retain their respective licenses; Cisco's included headers are covered by [their license](include/wels/LICENSE).

## Install

From this checkout, run once:

```bash
cargo run --locked --release --bin discord_stream_fix -- install
```

The installer downloads Cisco's OpenH264 2.5.1 library over HTTPS, verifies two SHA-256 checksums, and places the shim and supporting files in `~/.local/share/discord-stream-fix`. It creates two per-user systemd units that restore the shim at login and when Discord removes it. No system files, Discord settings, or desktop launchers are changed, and no sudo is required. The installer refuses to replace an unrecognized codec file or systemd unit.

Restart Discord if it was already running. Open it normally afterward, including from the application menu, autostart, or a Discord link.

The downloaded Cisco library is cached in `${XDG_CACHE_HOME:-~/.cache}/discord-stream-fix`. The installed executable and shim are independent of this checkout. Run the install command again after updating the project.

## Uninstall

```bash
~/.local/share/discord-stream-fix/discord-stream-fix uninstall
```

This removes the per-user units, installed files, and the codec cache entry if it still matches this shim. The downloaded codec in the user cache is retained. Discord may need a restart afterward.

## Development

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

The integration test compares decoded frames through Discord's FFmpeg wrapper with the native decoder. It requires the tested Discord installation, FFmpeg, and the `libx264` encoder; it reports a skip if Discord or FFmpeg is absent. The encoder forwarding test requires the verified Cisco library in the user cache and reports a skip if absent.

The project code is available under the [MIT license](LICENSE). The vendored Cisco headers are under their own BSD license.
