use bzip2::read::BzDecoder;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const NAME: &str = "libopenh264-2.5.1-linux64.7.so";
const URL: &str = "https://ciscobinary.openh264.org/libopenh264-2.5.1-linux64.7.so.bz2";
const ARCHIVE_HASH: &str = "82e436db8606433e3f823c7fff3269c43b3ba3f12510a851ef7b156b80ad1b11";
const LIBRARY_HASH: &str = "d828a944d4d2bb64195ada89cf2cde9bc41733b1547d0788ef49fb8cb231b76f";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(hex(&hasher.finalize()))
}

fn ensure_codec(source: &Path, target: &Path) -> io::Result<bool> {
    if target.exists() {
        return Ok(false);
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut fs::File::open(source)?, &mut file)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    match file.persist_noclobber(target) {
        Ok(_) => Ok(true),
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.error),
    }
}
fn replace_known_codec(source: &Path, target: &Path, allowed: &[&Path]) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent"))?;
    fs::create_dir_all(parent)?;
    if target.exists() {
        let existing = hash(target)?;
        if existing == hash(source)? {
            return Ok(());
        }
        let known = allowed
            .iter()
            .any(|path| path.is_file() && hash(path).is_ok_and(|h| h == existing));
        if !known {
            return Err(io::Error::other(format!(
                "{} has unexpected contents; refusing to replace it",
                target.display()
            )));
        }
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut fs::File::open(source)?, &mut file)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    file.persist(target).map_err(|e| e.error)?;
    Ok(())
}
fn obtain_codec(cache: &Path, previous: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(cache)?;
    let dest = cache.join(NAME);
    if dest.is_file() && hash(&dest)? == LIBRARY_HASH {
        return Ok(dest);
    }
    if previous.is_file() && hash(previous)? == LIBRARY_HASH {
        let mut file = tempfile::NamedTempFile::new_in(cache)?;
        io::copy(&mut fs::File::open(previous)?, &mut file)?;
        file.persist(&dest)?;
        return Ok(dest);
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client.get(URL).send()?.error_for_status()?;
    let mut archive = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut archive)?;
    if hex(&Sha256::digest(&archive)) != ARCHIVE_HASH {
        return Err("Cisco archive checksum mismatch".into());
    }
    let mut bytes = Vec::new();
    BzDecoder::new(archive.as_slice())
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if hex(&Sha256::digest(&bytes)) != LIBRARY_HASH {
        return Err("Cisco library checksum mismatch".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(cache)?;
    file.write_all(&bytes)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    file.persist(&dest)?;
    Ok(dest)
}
const MARKER: &str = "# Managed by discord-stream-fix";

fn unit_escape(path: &Path) -> io::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| io::Error::other("unit path is not UTF-8"))?;
    if value.contains(['\n', '\r']) {
        return Err(io::Error::other("unit path contains a newline"));
    }
    Ok(value
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace('"', "\\\""))
}
fn unit_contents(exe: &Path, shim: &Path, target: &Path) -> io::Result<(String, String)> {
    let parent = unit_escape(
        target
            .parent()
            .ok_or_else(|| io::Error::other("target has no parent"))?,
    )?;
    let exe = unit_escape(exe)?.replace('$', "$$");
    let shim = unit_escape(shim)?.replace('$', "$$");
    let target = unit_escape(target)?.replace('$', "$$");
    Ok((
        format!(
            "{MARKER}\n[Unit]\nDescription=Restore Discord H.264 decoder workaround\nAfter=discord-stream-fix.path\n\n[Service]\nType=oneshot\nExecStart=\"{exe}\" restore \"{shim}\" \"{target}\"\n\n[Install]\nWantedBy=default.target\n"
        ),
        format!(
            "{MARKER}\n[Unit]\nDescription=Watch Discord codec cache\n\n[Path]\nPathChanged={parent}\n\n[Install]\nWantedBy=default.target\n"
        ),
    ))
}
fn copy_atomic(source: &Path, target: &Path, mode: u32) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("target has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut fs::File::open(source)?, &mut file)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    file.persist(target).map_err(|e| e.error)?;
    Ok(())
}
fn write_unit(path: &Path, contents: &str) -> io::Result<()> {
    if path.exists() && !fs::read_to_string(path)?.starts_with(MARKER) {
        return Err(io::Error::other(format!(
            "{} is not managed by this tool",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("unit has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(contents.as_bytes())?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn systemctl(args: &[&str]) -> io::Result<()> {
    let uid = fs::metadata("/proc/self")?.uid();
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(|| PathBuf::from(format!("/run/user/{uid}")), PathBuf::from);
    let mut command = Command::new("systemctl");
    command
        .arg("--user")
        .args(args)
        .env("XDG_RUNTIME_DIR", &runtime);
    if env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        command.env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}/bus", runtime.display()),
        );
    }
    if !command.status()?.success() {
        return Err(io::Error::other(format!(
            "systemctl --user {} failed",
            args.join(" ")
        )));
    }
    Ok(())
}
fn paths(home: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let data = home.join(".local/share/discord-stream-fix");
    let config = env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let units = config.join("systemd/user");
    let target = config
        .join("discord/discord_asset_cache/openh264")
        .join(NAME);
    (data, units, target, config)
}
fn install(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cache = env::var_os("XDG_CACHE_HOME")
        .map_or_else(|| home.join(".cache"), PathBuf::from)
        .join("discord-stream-fix");
    let original = obtain_codec(&cache, &project.join("cache").join(NAME))?;
    let (data, units, target, _) = paths(home);
    let shim = project.join("target/release/libdiscord_stream_fix.so");
    let old = cache.join("previous-shim.so");
    if shim.is_file() && target.is_file() && hash(&shim)? == hash(&target)? {
        copy_atomic(&shim, &old, 0o644)?;
    }
    let status = Command::new("cargo")
        .args(["build", "--locked", "--release", "--lib", "--target-dir"])
        .arg(project.join("target"))
        .current_dir(&project)
        .status()?;
    if !status.success() {
        return Err("failed to build Rust decoder shim".into());
    }
    let installed_exe = data.join("discord-stream-fix");
    let installed_shim = data.join("decoder-shim.so");
    let installed_original = data.join("openh264-original.so");
    let (service, path) = unit_contents(&installed_exe, &installed_shim, &target)?;
    for name in ["discord-stream-fix.service", "discord-stream-fix.path"] {
        let file = units.join(name);
        if file.exists() && !fs::read_to_string(&file)?.starts_with(MARKER) {
            return Err(format!("{} is not managed by this tool", file.display()).into());
        }
    }
    // Check the current cache entry before writing an installed shim.
    if target.exists() {
        let existing = hash(&target)?;
        let installed_known = installed_shim.is_file() && hash(&installed_shim)? == existing;
        if existing != hash(&shim)?
            && existing != hash(&original)?
            && !(old.is_file() && hash(&old)? == existing)
            && !installed_known
        {
            return Err(format!(
                "{} has unexpected contents; refusing to replace it",
                target.display()
            )
            .into());
        }
        if installed_known {
            copy_atomic(&installed_shim, &old, 0o644)?;
        }
    }
    copy_atomic(&original, &installed_original, 0o644)?;
    copy_atomic(&shim, &installed_shim, 0o644)?;
    copy_atomic(&env::current_exe()?, &installed_exe, 0o755)?;
    replace_known_codec(&installed_shim, &target, &[&original, &old, &shim])?;
    write_unit(&units.join("discord-stream-fix.service"), &service)?;
    write_unit(&units.join("discord-stream-fix.path"), &path)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", "discord-stream-fix.path"])?;
    systemctl(&["enable", "--now", "discord-stream-fix.service"])?;
    println!("Installed. Start Discord normally; restart it if it is currently running.");
    Ok(())
}
fn uninstall(home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (data, units, target, _) = paths(home);
    for name in ["discord-stream-fix.service", "discord-stream-fix.path"] {
        let file = units.join(name);
        if file.exists() && !fs::read_to_string(&file)?.starts_with(MARKER) {
            return Err(format!("{} is not managed by this tool", file.display()).into());
        }
    }
    systemctl(&[
        "disable",
        "--now",
        "discord-stream-fix.path",
        "discord-stream-fix.service",
    ])?;
    for name in ["discord-stream-fix.service", "discord-stream-fix.path"] {
        let file = units.join(name);
        if file.exists() {
            fs::remove_file(file)?;
        }
    }
    systemctl(&["daemon-reload"])?;
    let shim = data.join("decoder-shim.so");
    if shim.is_file() && target.is_file() && hash(&target)? == hash(&shim)? {
        fs::remove_file(&target)?;
    }
    for name in [
        "discord-stream-fix",
        "decoder-shim.so",
        "openh264-original.so",
    ] {
        let file = data.join(name);
        if file.exists() {
            fs::remove_file(file)?;
        }
    }
    if data.is_dir() {
        let _ = fs::remove_dir(data);
    }
    println!("Uninstalled the Discord stream workaround.");
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() == 3 && args[0] == "restore" {
        ensure_codec(Path::new(&args[1]), Path::new(&args[2]))?;
        return Ok(());
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    if args.is_empty() || args == ["install"] {
        return install(&home);
    }
    if args == ["uninstall"] {
        return uninstall(&home);
    }
    if args == ["--help"] || args == ["-h"] {
        println!(
            "Usage: discord_stream_fix [install|uninstall]\n\nInstall or remove the per-user Discord stream workaround."
        );
        return Ok(());
    }
    Err("usage: discord_stream_fix [install|uninstall]".into())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn restores_missing_file_and_preserves_existing() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::write(&source, b"verified").unwrap();
        assert!(ensure_codec(&source, &target).unwrap());
        assert_eq!(fs::read(&target).unwrap(), b"verified");
        fs::write(&target, b"unchanged").unwrap();
        assert!(!ensure_codec(&source, &target).unwrap());
        assert_eq!(fs::read(&target).unwrap(), b"unchanged");
        fs::remove_file(&target).unwrap();
        assert!(ensure_codec(&source, &target).unwrap());
    }

    #[test]
    fn replaces_only_matching_known_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("shim");
        let original = root.path().join("original");
        let target = root.path().join("target");
        fs::write(&source, b"shim").unwrap();
        fs::write(&original, b"original").unwrap();
        fs::write(&target, b"original").unwrap();
        replace_known_codec(&source, &target, &[&original]).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"shim");
        fs::write(&target, b"unknown").unwrap();
        assert!(replace_known_codec(&source, &target, &[&original]).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"unknown");
    }

    #[test]
    fn units_escape_specifiers_and_preserve_paths() {
        let (service, path) = unit_contents(
            Path::new("/home/a b/discord%fix"),
            Path::new("/home/a b/shim.so"),
            Path::new("/home/a b/cache/codec.so"),
        )
        .unwrap();
        assert!(service.contains("\"/home/a b/discord%%fix\""));
        assert!(service.contains("\"/home/a b/cache/codec.so\""));
        assert!(path.contains("PathChanged=/home/a b/cache"));
        assert!(
            unit_contents(Path::new("/bad\nname"), Path::new("/s"), Path::new("/c/x")).is_err()
        );
    }

    #[test]
    fn refuses_to_replace_unmanaged_units() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("service");
        fs::write(&file, "user configuration").unwrap();
        assert!(write_unit(&file, MARKER).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "user configuration");
    }
}
