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

Press **F1** for shortcuts. **Right** opens the focused directory, **Left** goes up one directory, and **Ctrl+Q** toggles the focused file’s preview in the opposite panel. **Tab** switches panels, **Ctrl+L** edits the location, and **F5** and **F6** copy and move to the opposite panel.

Double-click a file or press **Enter** to open it. On Linux and macOS, executable binaries and scripts with a shebang run from their containing folder. Windows uses the system’s default action to launch programs. Images, documents, and other files open in the application associated with their type by the desktop.

Press **Ctrl+Shift+C** (**Cmd+Shift+C** on macOS) to copy the full path of the focused item or the full paths of all marked items.

Press **Ctrl+Up** to open the focused folder in a new tab in the same panel, or **Ctrl+Right** from the left panel to open it in a new tab on the right. **Ctrl+T** duplicates the current location in the same panel. **Ctrl+Shift+T** copies the current tab to the opposite panel.
Use **Ctrl+Tab** and **Ctrl+Shift+Tab** to cycle forward and backward through tabs in the active panel.
Right-click a tab to **Pin tab** or **Unpin tab**. Pinned tabs stay in their current positions, survive restarts, and are protected from closing and duplicate cleanup.

Press **F9** or choose **Tools → Open Terminal** to start an external terminal in the active panel's folder.

On Linux, choose **Tools → Add DeshCmd to Application Menu** or run `deshcmd --install-menu-entry` to install a launcher for your user account. The launcher points to the running executable or AppImage. Run the command again after moving it.

On Windows, choose **Tools → Add DeshCmd to Start Menu** or run the executable with `--install-menu-entry` to create a Start menu shortcut for your user account. Run the command again if you move the executable.

This version supports local files, mounted locations, archives, and virtual path lists. See [Features.md](Features.md) for the broader feature inventory. The deshcmd workspace and bookmark interchange format is JSON. The integrity tools support 22 checksum options, including SFV. HAVAL and native ACL editing remain outside this version. Trash, volume discovery, and system clipboard behavior depend on the desktop. Linux/X11 is the verified desktop target.

## Archives and packages

Install 7-Zip and put `7zz`, `7z`, or `7za` on your PATH. On Windows, DeshCmd also looks in the default 7-Zip installation folder. Set `DESHCMD_7ZIP` to use a specific executable.

- Press **Enter** or double-click an archive to browse it in the active panel. Use **Left** to go up or leave the archive. Press **Ctrl+Q** to preview a member. For supported Office, OpenDocument, and other ZIP-based documents, use **Browse archive** from the context menu to see their internal files.
- Choose **Tools → Archives → Unlock archive** to browse encrypted headers or preview encrypted files. The password is remembered for that archive until you close the app.
- **F5** copies selected members to a local folder or adds local files to the archive open in the opposite panel. **Tools → Archives** also contains **Pack**, **Extract**, **Delete archive members**, and **Test archive**.
- In **Pack**, choose the format by typing its extension in the destination path. ZIP and 7z support passwords and split volumes. To open a split archive, start with the `.001` file. In **Extract**, choose a mask, path preservation or flattening, and how existing files are handled. **Smart Extract** creates a containing folder when the matched members have multiple top-level items.

ZIP, 7z, and TAR support creation and member changes. TAR can also be packed with gzip, bzip2, XZ, LZMA, or Zstandard compression. The LZMA packer uses `xz`, Zstandard packing uses `zstd`, Brotli reads use `brotli`, and Base64 encoding uses `base64`. RAR uses UnRAR when available, with 7-Zip as the default when UnRAR is absent. Set `DESHCMD_UNRAR` to select the reader or `DESHCMD_RAR` to select the writer. Read-only formats and supported codecs depend on the installed archiver. DEB and RPM browsing opens the package payload. Archive extraction stages regular files and directories and skips links, special files, and unsafe paths.

### External archivers

Set `DESHCMD_ARCHIVERS` to a JSON configuration path and restart DeshCmd to register extensions or override existing handlers. Each action is an argument array passed directly to the configured executable.

```json
[
  {
    "extensions": ["example"],
    "executable": "/path/to/archiver-adapter",
    "listing": "json",
    "commands": {
      "list": ["list", "{archive}"],
      "extract": ["extract-to-stdout", "{archive}", "{member}"],
      "pack": ["pack", "{archive}", "{input}"],
      "delete": ["delete", "{archive}", "{input}"],
      "test": ["test", "{archive}"]
    }
  }
]
```

The list action can return a JSON array of members with `path`, optional `size`, and optional booleans `folder` and `link`. With `"listing": "technical"`, it returns the same record format as `7z l -slt -ba`. The extract action writes one member’s bytes to stdout. The pack action receives a staged directory as `{input}`, and delete receives a UTF-8 file with one selected member path per line. Add `{password}` as an argument if your adapter supports it. Omit unsupported actions from the configuration.

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
