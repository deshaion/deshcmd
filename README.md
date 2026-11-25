# DeshCmd

DeshCmd is a native dual-pane file manager for local files and mounted volumes. The desktop uses Rust, egui 0.37, and wgpu. The headless engine owns workspace state and runs file operations on a background queue.

## Build and run

Install Rust 1.95 or later and a C toolchain for the native dependencies. On Linux, run under Wayland or X11 with a wgpu-compatible graphics driver. For example, Debian and Ubuntu systems can use `build-essential`, `pkg-config`, `libvulkan1`, and the Vulkan driver for their GPU. Native file pickers use the desktop's XDG portal or a desktop dialog fallback. The Linux unmount action uses `gio`, provided by `libglib2.0-bin` on Debian-based systems.

```sh
cargo run -p deshcmd
cargo run -p deshcmd -- /path/to/source /path/to/destination
```

An ordinary launch restores the saved workspace. Passing directories opens a fresh workspace with those panel paths.

## Download builds

The **Build desktop packages** workflow runs when a GitHub release is published and attaches the packages to that release. Download them from the release's **Assets** section. Filenames use the Cargo workspace version.

| Platform | Package for version 0.1.0 |
| --- | --- |
| Linux x86-64 | `deshcmd-0.1.0-x86_64.AppImage` |
| Windows x86-64 | `deshcmd-0.1.0-x86_64.exe` |
| macOS Apple Silicon | `deshcmd-0.1.0-aarch64.dmg` |
| macOS Intel | `deshcmd-0.1.0-x86_64.dmg` |

On Linux, mark the downloaded AppImage executable with `chmod +x deshcmd-0.1.0-x86_64.AppImage`, then run it. On Windows, launch the `.exe` directly. On macOS, open the disk image and drag `DeshCmd.app` into Applications. The macOS app is ad hoc signed. Gatekeeper may block downloaded copies until a build is signed with a Developer ID and notarized.

## What is available

- Independent panels with tabs, tab pins and locks, named workspaces, bookmarks, breadcrumbs, history, a folder tree, and mounted volumes. The panel divider is draggable. Layout can be side by side or stacked.
- Details, compact, gallery, and recursive flat views. Quick View shows common images and UTF-8 text. Quick Search jumps to matching names, while the filter narrows the visible list.
- Keyboard and mouse marking, mask and attribute matching, reusable selections, clipboard actions, and import/export of path lists and displayed details.
- Background copy and move with conflict choices, symlink options, masks, and copy verification. Create, rename, trash, delete, link, or change file attributes, and inspect logs and progress from **Queue**.
- Split and join files, generate and verify checksum manifests, compare a file with a pasted digest, and benchmark supported hash algorithms.

Press **F1** for shortcuts. In the default keymap, **Tab** switches panels, **Ctrl+L** edits the location, and **F5** and **F6** copy and move to the opposite panel.
Press **Ctrl+Up** to open the focused folder in a new tab in the same panel, or **Ctrl+Right** from the left panel to open it in a new tab on the right. **Ctrl+T** duplicates the current location in the same panel. **Ctrl+Shift+T** copies the current tab to the opposite panel.
Right-click a tab to **Pin tab** or **Unpin tab**. Pinned tabs stay in their current positions, survive restarts, and are protected from closing and duplicate cleanup.
Press **F9** or choose **Tools → Open Terminal** to start an external terminal in the active panel's folder.

On Linux, choose **Tools → Add DeshCmd to Application Menu** or run `deshcmd --install-menu-entry` to install a launcher for your user account. The launcher points to the running executable or AppImage. Run the command again after moving it.

On Windows, choose **Tools → Add DeshCmd to Start Menu** or run the executable with `--install-menu-entry` to create a Start menu shortcut for your user account. Run the command again if you move the executable.

This version supports local files, mounted locations, and virtual path lists. Archive and plugin-backed volumes in [Features.md](Features.md). The deshcmd workspace and bookmark interchange format is JSON. The integrity tools support 22 checksum options, including SFV. HAVAL and native ACL editing remain outside this version. Trash, volume discovery, and system clipboard behavior depend on the desktop. Linux/X11 is the verified desktop target.

## Workspace

- `apps/deshcmd-desktop` owns the window, native dialogs, clipboard, and operating-system integration.
- `crates/deshcmd-ui` owns presentation, shortcuts, dialogs, and preview textures.
- `crates/deshcmd-engine` owns navigation, filtering, selections, and file operations.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --release -p deshcmd
```
