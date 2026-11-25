#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod menu;
mod terminal;
#[cfg(feature = "standalone-updater")]
mod updater;

use std::path::{Path, PathBuf};

use deshcmd_ui::{App, Desktop, Place};
use eframe::egui;

mod launch;

struct NativeDesktop {
    clipboard: Option<arboard::Clipboard>,
}

impl NativeDesktop {
    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        self.clipboard
            .as_mut()
            .ok_or_else(|| "System clipboard unavailable".into())
    }
}

impl Desktop for NativeDesktop {
    #[cfg(feature = "standalone-updater")]
    fn supports_updates(&self) -> bool {
        updater::available()
    }
    #[cfg(feature = "standalone-updater")]
    fn check_for_update(&mut self, ctx: egui::Context) -> Result<deshcmd_ui::UpdateCheck, String> {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("deshcmd-update".into())
            .spawn(move || {
                let result = updater::prepare()
                    .map(|update| {
                        update.map(|prepared| deshcmd_ui::DesktopUpdate {
                            version: prepared.version.clone(),
                            install: Box::new(move || {
                                prepared
                                    .schedule_install()
                                    .map_err(|error| error.to_string())
                            }),
                        })
                    })
                    .map_err(|error| error.to_string());
                let _ = send.send(result);
                ctx.request_repaint();
            })
            .map_err(|error| error.to_string())?;
        Ok(receive)
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        launch::open_file(path).map_err(|error| format!("{}: {error}", path.display()))
    }
    fn open_terminal(&mut self, directory: &Path) -> Result<(), String> {
        terminal::open(directory)
    }
    #[cfg(target_os = "linux")]
    fn install_menu_entry(&mut self) -> Result<(), String> {
        menu::install()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(windows)]
    fn install_menu_entry(&mut self) -> Result<(), String> {
        install_start_menu_entry()
    }
    fn pick_file(&mut self, initial: &Path, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_directory(initial)
            .set_title(title)
            .pick_file()
    }
    fn save_file(&mut self, initial: &Path, name: &str, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_directory(initial)
            .set_file_name(name)
            .set_title(title)
            .save_file()
    }
    fn pick_directory(&mut self, initial: &Path, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_directory(initial)
            .set_title(title)
            .pick_folder()
    }
    fn copy_files(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        self.clipboard()?
            .set()
            .file_list(paths)
            .map_err(|error| error.to_string())
    }
    fn clipboard_files(&mut self) -> Vec<PathBuf> {
        self.clipboard()
            .ok()
            .and_then(|clipboard| clipboard.get().file_list().ok())
            .unwrap_or_default()
    }
    fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard().ok()?.get_text().ok()
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard()?
            .set_text(text)
            .map_err(|error| error.to_string())
    }
    fn places_reader(&self) -> fn() -> Vec<Place> {
        read_places
    }
    fn eject(&mut self, path: &Path) -> Result<(), String> {
        unmount(path)
    }
    fn shortcut(&mut self, target: &Path, source: &Path) -> Result<(), String> {
        shortcut(target, source)
    }
}

fn read_places() -> Vec<Place> {
    let known = [
        ("Desktop", dirs::desktop_dir()),
        ("Documents", dirs::document_dir()),
        ("Downloads", dirs::download_dir()),
        ("Pictures", dirs::picture_dir()),
        ("Music", dirs::audio_dir()),
        ("Videos", dirs::video_dir()),
    ];
    let mut result = known
        .into_iter()
        .filter_map(|(name, path)| {
            let path = path?.canonicalize().ok()?;
            path.is_dir().then_some(Place {
                name: name.into(),
                path,
                drive: false,
                ejectable: false,
            })
        })
        .collect::<Vec<_>>();
    result.extend(volumes());
    result
}

#[cfg(target_os = "linux")]
fn volumes() -> Vec<Place> {
    parse_mounts(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default())
}

#[cfg(target_os = "linux")]
fn parse_mounts(mountinfo: &str) -> Vec<Place> {
    use std::os::unix::ffi::OsStringExt;
    let mut seen = std::collections::HashSet::new();
    let mut mounts = mountinfo
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let escaped = left.split_whitespace().nth(4)?;
            let mut fields = right.split_whitespace();
            let fs = fields.next()?;
            let device = fields.next()?;
            let mut decoded = Vec::new();
            let mut rest = escaped.as_bytes();
            while let Some((&byte, suffix)) = rest.split_first() {
                if byte == b'\\'
                    && suffix.len() >= 3
                    && suffix[..3].iter().all(|digit| matches!(digit, b'0'..=b'7'))
                {
                    let value = u16::from(suffix[0] - b'0') * 64
                        + u16::from(suffix[1] - b'0') * 8
                        + u16::from(suffix[2] - b'0');
                    if let Ok(value) = u8::try_from(value) {
                        decoded.push(value);
                        rest = &suffix[3..];
                        continue;
                    }
                }
                decoded.push(byte);
                rest = suffix;
            }
            let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));
            let removable = ["/run/media", "/media", "/mnt", "/run/user"]
                .iter()
                .any(|root| path.starts_with(root));
            let remote = matches!(
                fs,
                "nfs" | "nfs4" | "cifs" | "smb3" | "fuse.sshfs" | "fuse.rclone" | "fuse.gvfsd-fuse"
            );
            let visible =
                path == Path::new("/") || remote || removable && device.starts_with("/dev/");
            if !visible || !seen.insert(path.clone()) {
                return None;
            }
            let name = if path == Path::new("/") {
                "Filesystem".into()
            } else {
                deshcmd_engine::display_name(&path)
            };
            Some(Place {
                name,
                ejectable: removable && device.starts_with("/dev/"),
                path,
                drive: true,
            })
        })
        .collect::<Vec<_>>();
    if !seen.contains(Path::new("/")) {
        mounts.push(root_volume());
    }
    mounts.sort_by_key(|volume| (volume.path != Path::new("/"), volume.name.to_lowercase()));
    mounts
}

#[cfg(target_os = "macos")]
fn volumes() -> Vec<Place> {
    let mut places = vec![root_volume()];
    for entry in std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_dir()
            && path
                .canonicalize()
                .ok()
                .is_some_and(|path| path != Path::new("/"))
        {
            places.push(Place {
                name: deshcmd_engine::display_name(&path),
                path,
                drive: true,
                ejectable: true,
            });
        }
    }
    places
}

#[cfg(windows)]
fn volumes() -> Vec<Place> {
    (b'A'..=b'Z')
        .filter_map(|letter| {
            let name = format!("{}:", char::from(letter));
            let path = PathBuf::from(format!("{name}\\"));
            path.is_dir().then_some(Place {
                name,
                path,
                drive: true,
                ejectable: false,
            })
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn volumes() -> Vec<Place> {
    vec![root_volume()]
}

#[cfg(not(windows))]
fn root_volume() -> Place {
    Place {
        name: "Filesystem".into(),
        path: PathBuf::from("/"),
        drive: true,
        ejectable: false,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn unmount(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = std::process::Command::new("gio");
        command.args(["mount", "--unmount"]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("diskutil");
        command.arg("unmount");
        command
    };
    let result = command
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;
    if result.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&result.stderr);
    Err(if error.trim().is_empty() {
        format!("Unmount returned {}", result.status)
    } else {
        error.trim().to_owned()
    })
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unmount(_path: &Path) -> Result<(), String> {
    Err("Unmount is unavailable in this build".into())
}

#[cfg(windows)]
fn shortcut(path: &Path, source: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_ok()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
    {
        return Err("Choose a new path ending in .lnk".into());
    }
    write_windows_shortcut(Some(path), source)
}

#[cfg(windows)]
fn install_start_menu_entry() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    write_windows_shortcut(None, &executable)
}

#[cfg(windows)]
fn write_windows_shortcut(path: Option<&Path>, source: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let script = r#"
$ErrorActionPreference = 'Stop'
$path = $env:DESHCMD_LINK
$installInStartMenu = [string]::IsNullOrEmpty($path)
if ($installInStartMenu) {
    $programs = [Environment]::GetFolderPath(
        [Environment+SpecialFolder]::Programs,
        [Environment+SpecialFolderOption]::Create
    )
    if ([string]::IsNullOrEmpty($programs)) { throw 'The Start menu Programs folder is unavailable.' }
    $path = Join-Path $programs 'DeshCmd.lnk'
}
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($path)
$link.TargetPath = $env:DESHCMD_SOURCE
if ($installInStartMenu) {
    $link.Arguments = ''
    $link.WorkingDirectory = [System.IO.Path]::GetDirectoryName($env:DESHCMD_SOURCE)
}
$link.Save()
"#;
    let mut command = std::process::Command::new("powershell.exe");
    command
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("DESHCMD_SOURCE", source);
    if let Some(path) = path {
        command.env("DESHCMD_LINK", path);
    } else {
        command.env_remove("DESHCMD_LINK");
    }
    let result = command.output().map_err(|error| error.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&result.stderr);
        Err(if error.trim().is_empty() {
            format!("Shortcut creation returned {}", result.status)
        } else {
            error.trim().to_owned()
        })
    }
}
#[cfg(not(windows))]
fn shortcut(_path: &Path, _source: &Path) -> Result<(), String> {
    Err("Windows shortcuts are only available on Windows".into())
}

fn icon() -> egui::IconData {
    let width: usize = 64;
    let mut rgba = vec![0; width * width * 4];
    for y in 0..width {
        for x in 0..width {
            let dx = x.abs_diff(32).saturating_sub(17);
            let dy = y.abs_diff(32).saturating_sub(17);
            let color = if dx * dx + dy * dy > 12 * 12 {
                [0, 0, 0, 0]
            } else if ((16..30).contains(&x) && (16..48).contains(&y))
                || ((34..48).contains(&x) && (21..43).contains(&y))
            {
                [12, 15, 19, 255]
            } else {
                [170, 235, 129, 255]
            };
            let start = (y * width + x) * 4;
            rgba[start..start + 4].copy_from_slice(&color);
        }
    }
    egui::IconData {
        rgba,
        width: width as u32,
        height: width as u32,
    }
}

fn requested_directories(
    cwd: &Path,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> [Option<PathBuf>; 2] {
    std::array::from_fn(|_| args.next().map(|path| cwd.join(path)))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1).peekable();
    if args
        .peek()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!("deshcmd [LEFT_DIRECTORY [RIGHT_DIRECTORY]]");
        #[cfg(any(target_os = "linux", windows))]
        println!("deshcmd --install-menu-entry");
        #[cfg(feature = "standalone-updater")]
        if updater::available() {
            println!("deshcmd --schedule-update  (install after this process exits)");
        }
        return Ok(());
    }
    #[cfg(not(feature = "standalone-updater"))]
    if args.peek().is_some_and(|arg| {
        arg == "--schedule-update"
            || arg == "--update"
            || arg == "--install-update"
            || arg == "--finish-update"
    }) {
        return Err("Standalone updates are unavailable in this installation".into());
    }
    #[cfg(feature = "standalone-updater")]
    let update_status = match args.peek().and_then(|arg| arg.to_str()) {
        Some("--schedule-update" | "--update") => {
            args.next();
            if args.next().is_some() {
                return Err("--schedule-update takes no arguments".into());
            }
            if let Some(update) = updater::prepare()? {
                let version = update.version.clone();
                update.schedule_install()?;
                println!("DeshCmd {version} is scheduled to install after this process exits.");
            } else {
                println!("DeshCmd is up to date");
            }
            return Ok(());
        }
        Some("--install-update") => return updater::install_cli(args.skip(1)),
        Some("--finish-update") => updater::finish(args.by_ref().skip(1))?,
        _ => None,
    };
    #[cfg(any(target_os = "linux", windows))]
    if args.peek().is_some_and(|arg| arg == "--install-menu-entry") {
        if args.nth(1).is_some() {
            return Err("--install-menu-entry takes no arguments".into());
        }
        #[cfg(target_os = "linux")]
        println!(
            "Added DeshCmd to the application menu at {}",
            menu::install()?.display()
        );
        #[cfg(windows)]
        install_start_menu_entry()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if std::env::var_os("WINIT_X11_SCALE_FACTOR").is_none() {
        // Use a consistent pixel scale across X11 monitors. Interface zoom still applies.
        // SAFETY: This runs at process startup, before any threads are started.
        unsafe { std::env::set_var("WINIT_X11_SCALE_FACTOR", "1") };
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = dirs::home_dir().unwrap_or_else(|| cwd.clone());
    let fresh = args.peek().is_some();
    let requested = requested_directories(&cwd, args);
    let initial = [cwd.clone(), cwd];
    let viewport = egui::ViewportBuilder::default()
        .with_title("deshcmd")
        .with_app_id("deshcmd")
        .with_inner_size([1220., 540.])
        .with_icon(icon());
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_decorations(false);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native(
        "deshcmd",
        options,
        Box::new(move |context| {
            let mut app = App::new(
                context,
                Box::new(NativeDesktop { clipboard: None }),
                home,
                initial,
                fresh,
            );
            app.open_initial_directories(&context.egui_ctx, requested);
            #[cfg(feature = "standalone-updater")]
            if let Some(error) = update_status {
                app.show_update_error(error);
            }
            // Keep the native window's minimum independent of interface zoom.
            let ctx = &context.egui_ctx;
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
                egui::vec2(900., 480.) / ctx.zoom_factor(),
            ));
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
