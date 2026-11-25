use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ring::{digest, signature};
use semver::Version;
use serde::Deserialize;
use tempfile::TempDir;

const RELEASES: &str = "https://github.com/deshaion/deshcmd/releases";
const STAGE_PREFIX: &str = ".deshcmd-update-";
const STAGE_RANDOM_LEN: usize = 12;
const HELPER: &str = if cfg!(windows) {
    "deshcmd-updater.exe"
} else {
    "deshcmd-updater"
};
const MANIFEST: &str = "latest.json";
const SIGNATURE: &str = "latest.json.sig";
const STAGED_BACKUP: &str = "backup";
const LIMIT: u64 = 256 * 1024 * 1024;

type Error = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize)]
struct Manifest {
    schema: u32,
    version: String,
    tag: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    sha256: String,
    executable_sha256: String,
}

struct VerifiedUpdate {
    version: String,
    tag: String,
    asset: Asset,
}

fn platform_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("deshcmd-linux-x86_64.tar.gz"),
        ("linux", "aarch64") => Some("deshcmd-linux-aarch64.tar.gz"),
        ("windows", "x86_64") => Some("deshcmd-windows-x86_64.zip"),
        ("macos", "aarch64") => Some("deshcmd-macos-aarch64.tar.gz"),
        ("macos", "x86_64") => Some("deshcmd-macos-x86_64.tar.gz"),
        _ => None,
    }
}

pub fn available() -> bool {
    install_target().is_ok()
}

fn public_key() -> Result<[u8; 32]> {
    let key = option_env!("DESHCMD_UPDATE_PUBLIC_KEY").ok_or("This build has no update key")?;
    let mut bytes = [0; 32];
    hex::decode_to_slice(key, &mut bytes)?;
    Ok(bytes)
}

fn managed_environment() -> bool {
    ["APPIMAGE", "FLATPAK_ID", "SNAP"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
        || cfg!(target_os = "linux") && Path::new("/.flatpak-info").exists()
}

fn install_target() -> Result<PathBuf> {
    if platform_asset().is_none() || public_key().is_err() || managed_environment() {
        return Err("Standalone updates are unavailable in this installation".into());
    }
    let target = fs::canonicalize(std::env::current_exe()?)?;
    if ["/bin", "/usr/bin", "/nix/store", "/gnu/store"]
        .iter()
        .any(|root| target.starts_with(root))
        || target.ancestors().any(|path| {
            path.extension().is_some_and(|ext| ext == "app")
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        matches!(name, "Cellar" | "Caskroom") || is_stage_name(name)
                    })
        })
    {
        return Err("Standalone updates require an extracted executable".into());
    }
    if fs::metadata(&target)?.permissions().readonly() {
        return Err("The DeshCmd executable is read-only".into());
    }
    Ok(target)
}

fn authenticate(
    bytes: &[u8],
    sig: &[u8],
    key: &[u8; 32],
    current: &Version,
    asset_name: &str,
) -> Result<Option<VerifiedUpdate>> {
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(bytes, sig)
        .map_err(|_| "Invalid DeshCmd update signature")?;
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    let version = Version::parse(&manifest.version)?;
    if manifest.schema != 1
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || (manifest.tag != manifest.version && manifest.tag != format!("v{version}"))
    {
        return Err("Unsupported DeshCmd update manifest".into());
    }
    if version.cmp_precedence(current).is_le() {
        return Ok(None);
    }
    let mut matching = manifest
        .assets
        .into_iter()
        .filter(|asset| asset.name == asset_name);
    let asset = matching
        .next()
        .ok_or("The update has no archive for this platform")?;
    if matching.next().is_some() || asset.size == 0 || asset.size > LIMIT {
        return Err("Invalid update archive metadata".into());
    }
    Ok(Some(VerifiedUpdate {
        version: manifest.version,
        tag: manifest.tag,
        asset,
    }))
}

fn download(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
    destination: &mut impl Write,
) -> Result<u64> {
    let mut response = agent.get(url).call()?;
    let size = io::copy(
        &mut response.body_mut().as_reader().take(limit + 1),
        destination,
    )?;
    if size > limit {
        return Err("Update download exceeds its size limit".into());
    }
    Ok(size)
}

fn sha256(mut input: impl Read) -> Result<digest::Digest> {
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(context.finish());
        }
        context.update(&buffer[..count]);
    }
}

fn verify_hash(file: &Path, expected: &str) -> Result<()> {
    if hex::encode(sha256(File::open(file)?)?) != expected {
        return Err("DeshCmd update SHA-256 mismatch".into());
    }
    Ok(())
}

fn extract(archive: File, output: &mut File) -> Result<()> {
    #[cfg(not(windows))]
    {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        let mut entries = archive.entries()?;
        let mut entry = entries.next().ok_or("Empty update archive")??;
        if !entry.header().entry_type().is_file()
            || entry.path()?.as_ref() != Path::new("deshcmd")
            || entry.size() > LIMIT
        {
            return Err("Invalid executable in update archive".into());
        }
        copy_executable(&mut entry, output)?;
        if entries.next().is_some() {
            return Err("Unexpected entry in update archive".into());
        }
    }
    #[cfg(windows)]
    {
        let mut archive = zip::ZipArchive::new(archive)?;
        if archive.len() != 1 {
            return Err("Expected one executable in update archive".into());
        }
        let mut entry = archive.by_index(0)?;
        if !entry.is_file()
            || entry.is_symlink()
            || entry.name() != "deshcmd.exe"
            || entry.size() > LIMIT
        {
            return Err("Invalid executable in update archive".into());
        }
        copy_executable(&mut entry, output)?;
    }
    Ok(())
}

fn copy_executable(input: &mut impl Read, output: &mut File) -> Result<()> {
    let size = io::copy(&mut input.take(LIMIT + 1), output)?;
    if size == 0 || size > LIMIT {
        return Err("Invalid executable size in update archive".into());
    }
    Ok(())
}

pub struct Prepared {
    pub version: String,
    target: PathBuf,
    stage: TempDir,
}

impl Prepared {
    pub fn schedule_install(self) -> Result<()> {
        let helper = self.stage.path().join(HELPER);
        Command::new(helper)
            .arg("--install-update")
            .arg(&self.target)
            .arg(std::process::id().to_string())
            .spawn()?;
        let _ = self.stage.keep();
        Ok(())
    }
}

fn running_image() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    return Ok(PathBuf::from("/proc/self/exe"));
    #[cfg(not(target_os = "linux"))]
    std::env::current_exe()
}

pub fn prepare() -> Result<Option<Prepared>> {
    let target = install_target()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(180)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(150)))
        .build()
        .into();
    let mut bytes = Vec::new();
    let mut sig = Vec::new();
    download(
        &agent,
        &format!("{RELEASES}/latest/download/{MANIFEST}"),
        64 * 1024,
        &mut bytes,
    )?;
    download(
        &agent,
        &format!("{RELEASES}/latest/download/{SIGNATURE}"),
        64,
        &mut sig,
    )?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let name = platform_asset().ok_or("Unsupported update platform")?;
    let Some(update) = authenticate(&bytes, &sig, &public_key()?, &current, name)? else {
        return Ok(None);
    };
    let asset = update.asset;

    let parent = target
        .parent()
        .ok_or("Missing DeshCmd installation folder")?;
    let stage = tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .rand_bytes(STAGE_RANDOM_LEN)
        .tempdir_in(parent)?;
    let archive = stage.path().join(&asset.name);
    let url = format!("{RELEASES}/download/{}/{}", update.tag, asset.name);
    if download(&agent, &url, asset.size, &mut File::create(&archive)?)? != asset.size {
        return Err("Incomplete DeshCmd update download".into());
    }
    verify_hash(&archive, &asset.sha256)?;
    let executable = stage.path().join("replacement");
    let mut output = File::create(&executable)?;
    extract(File::open(&archive)?, &mut output)?;
    set_executable_permissions(&target, &output)?;
    output.sync_all()?;
    verify_hash(&executable, &asset.executable_sha256)?;
    fs::remove_file(archive)?;
    fs::write(stage.path().join(MANIFEST), bytes)?;
    fs::write(stage.path().join(SIGNATURE), sig)?;
    fs::copy(running_image()?, stage.path().join(HELPER))?;
    Ok(Some(Prepared {
        version: update.version,
        target,
        stage,
    }))
}

fn set_executable_permissions(target: &Path, output: &File) -> io::Result<()> {
    let permissions = fs::metadata(target)?.permissions();
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(permissions.mode() & 0o777)
    };
    output.set_permissions(permissions)
}

pub fn install_cli(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    let target = PathBuf::from(args.next().ok_or("Missing update destination")?);
    let pid = parse_pid(args.next())?;
    if args.next().is_some() {
        return Err("Unexpected updater arguments".into());
    }
    let helper = fs::canonicalize(std::env::current_exe()?)?;
    let stage = helper.parent().ok_or("Missing update staging folder")?;
    if helper.file_name().is_none_or(|name| name != HELPER)
        || stage
            .file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| !is_stage_name(name))
        || stage.parent() != target.parent()
        || fs::canonicalize(&target)? != target
    {
        return Err("Invalid update staging folder or destination".into());
    }

    wait_for_parent(pid)?;
    let result = public_key().and_then(|key| replace_and_restart(&target, stage, &helper, &key));
    if let Err(error) = result {
        let message = format!("Could not install the DeshCmd update. {error}");
        let _ = restart(&target, stage, Some(&message));
        return Err(message.into());
    }
    Ok(())
}

fn replace_and_restart(target: &Path, stage: &Path, helper: &Path, key: &[u8; 32]) -> Result<()> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(target.with_file_name(".deshcmd-update.lock"))?;
    lock.lock()?;
    if sha256(File::open(target)?)?.as_ref() != sha256(File::open(helper)?)?.as_ref() {
        return Err("The installed DeshCmd executable changed before replacement".into());
    }
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let name = platform_asset().ok_or("Unsupported update platform")?;
    let manifest = fs::read(stage.join(MANIFEST))?;
    let sig = fs::read(stage.join(SIGNATURE))?;
    let Some(update) = authenticate(&manifest, &sig, key, &current, name)? else {
        return Err("The staged update is not newer than the installed version".into());
    };
    let replacement = stage.join("replacement");
    verify_hash(&replacement, &update.asset.executable_sha256)?;

    let mut backup_name = target
        .file_name()
        .ok_or("Missing executable name")?
        .to_os_string();
    backup_name.push(".previous");
    let backup = target.with_file_name(backup_name);
    let backup_copy = stage.join(STAGED_BACKUP);
    let replace = (|| {
        fs::copy(target, &backup_copy)?;
        OpenOptions::new()
            .write(true)
            .open(&backup_copy)?
            .sync_all()?;
        fs::rename(replacement, target)
    })();
    if let Err(error) = replace {
        let _ = fs::remove_file(&backup_copy);
        return Err(error.into());
    }
    if let Err(error) = restart(target, stage, None) {
        fs::rename(&backup_copy, target).map_err(|rollback| {
            format!(
                "Launch failed ({error}). Restore {} manually ({rollback}).",
                backup_copy.display()
            )
        })?;
        return Err(format!(
            "The new executable could not launch. Restored the previous version. {error}"
        )
        .into());
    }
    // The restarted process waits for this helper before checking the staging
    // directory. If promotion fails, it will report where the backup remains.
    if let Err(error) = fs::rename(&backup_copy, &backup) {
        eprintln!(
            "Could not move the update backup to {}. {error}",
            backup.display()
        );
    }
    Ok(())
}

fn restart(target: &Path, stage: &Path, error: Option<&str>) -> io::Result<()> {
    let mut command = Command::new(target);
    command
        .arg("--finish-update")
        .arg(stage.file_name().expect("validated stage"))
        .arg(std::process::id().to_string());
    if let Some(error) = error {
        command.arg(error);
    }
    command.spawn()?;
    Ok(())
}

pub fn finish(mut args: impl Iterator<Item = OsString>) -> Result<Option<String>> {
    let stage = args.next().ok_or("Missing update staging folder")?;
    let name = stage
        .to_str()
        .filter(|name| is_stage_name(name))
        .ok_or("Invalid update staging folder")?;
    let pid = parse_pid(args.next())?;
    let mut message = args.next().map(|text| text.to_string_lossy().into_owned());
    if args.next().is_some() {
        return Err("Unexpected updater arguments".into());
    }
    let current = fs::canonicalize(std::env::current_exe()?)?;
    let stage = current.with_file_name(name);
    if fs::symlink_metadata(&stage).is_ok_and(|meta| meta.is_dir() && !meta.is_symlink())
        && fs::canonicalize(&stage)? == stage
    {
        wait_for_parent(pid)?;
        // Delete only files created by the updater, even if a folder with the
        // expected name was supplied from the command line.
        for file in [HELPER, MANIFEST, SIGNATURE, "replacement"] {
            let _ = fs::remove_file(stage.join(file));
        }
        let backup = stage.join(STAGED_BACKUP);
        if message.is_none() && backup.is_file() {
            message = Some(format!(
                "DeshCmd updated successfully. The backup is at {}.",
                backup.display()
            ));
        }
        let _ = fs::remove_dir(stage);
    }
    Ok(message)
}

fn is_stage_name(name: &str) -> bool {
    name.strip_prefix(STAGE_PREFIX).is_some_and(|suffix| {
        suffix.len() == STAGE_RANDOM_LEN && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn parse_pid(arg: Option<OsString>) -> Result<u32> {
    Ok(arg
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .filter(|pid| *pid > 0)
        .ok_or("Invalid updater process ID")?)
}

#[cfg(unix)]
fn wait_for_parent(pid: u32) -> io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    // getppid changes when the parent exits, including when it becomes a zombie.
    // SAFETY: getppid takes no arguments and has no side effects.
    while unsafe { libc::getppid() } as u32 == pid {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DeshCmd did not exit",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(windows)]
fn wait_for_parent(pid: u32) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    // SAFETY: OpenProcess returns an owned handle on success. Wait and close
    // use this handle before it is discarded.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                Ok(())
            } else {
                Err(error)
            };
        }
        let result = match WaitForSingleObject(process, 90_000) {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DeshCmd did not exit",
            )),
            _ => Err(io::Error::last_os_error()),
        };
        CloseHandle(process);
        result
    }
}

#[cfg(test)]
mod tests;
