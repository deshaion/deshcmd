use std::path::{Component, Path, PathBuf};

use deshcmd_engine::{
    self as engine, Algorithm, ArchiveOperation, ArchiveSelection, Bookmark, Collision, Column,
    Command, Entry, ExtractLayout, ExtractOptions, ManifestMode, MatchOptions, Operation,
    PackOptions, Side, TransferOptions,
};
use eframe::egui;
use egui::{Align, Id, Layout, TextEdit, Ui};

use crate::{Action, App, COPY_PATH_SHORTCUT, FileClipboard, theme::*};

#[derive(Clone, Copy)]
pub(crate) enum Tool {
    OpenTerminal,
    BrowseArchive,
    UnlockArchive,
    OpenAssociated,
    Pack,
    Unpack,
    DeleteArchiveMembers,
    TestArchive,
    #[cfg(any(target_os = "linux", windows))]
    InstallMenuEntry,
    NewFolder,
    NewFile,
    Copy,
    CopyHere,
    Move,
    Rename,
    Trash,
    Delete,
    Wipe,
    Symlink,
    Hardlink,
    Shortcut,
    Inspect,
    ChangeAttributes,
    Comment,
    Statistics,
    Split,
    Combine,
    Hash,
    Verify,
    Benchmark,
    CopyClipboard,
    CutClipboard,
    Paste,
    CopyNames,
    CopyPaths,
    CopyParentPaths,
    CopyDetails,
    ExportDetails,
    ExportMarks,
    MarksFile,
    MarksClipboard,
    ListFile,
    ListClipboard,
    Find,
    FilterOptions,
    MarkMask,
    UnmarkMask,
    Bookmark,
    EditBookmark(usize),
    ImportBookmarks,
    ExportBookmarks,
    FolderPicker,
    FolderInOtherPane,
    RenameTab(usize),
    SaveFavorite,
    Favorites,
    ImportTabs,
    ExportTabs,
    Settings,
    JobDetails(u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Pack,
    Unpack,
    DeleteArchiveMembers,
    TestArchive,
    UnlockArchive,
    Transfer,
    Delete,
    Wipe,
    Inspect,
    Attributes,
    Split,
    Combine,
    Hash,
    Verify,
    Criteria,
    Bookmark,
    Favorites,
    HelpDetail,
    Eject,
}

pub(crate) struct Dialog {
    kind: Kind,
    tool: Tool,
    side: Side,
    paths: Vec<PathBuf>,
    title: String,
    description: String,
    primary: String,
    secondary: String,
    group: String,
    options: TransferOptions,
    algorithm: Algorithm,
    manifest: ManifestMode,
    numeric: String,
    flags: [bool; 4],
    criteria: MatchOptions,
    pack: PackOptions,
    extract: ExtractOptions,
    archives: Vec<ArchiveSelection>,
    archive_target: Option<(PathBuf, PathBuf)>,
    focus: bool,
    focus_footer: bool,
    error: Option<String>,
    validating: Option<(u64, u64)>,
}

impl Dialog {
    fn new(kind: Kind, tool: Tool, side: Side, title: &str, paths: Vec<PathBuf>) -> Self {
        Self {
            kind,
            tool,
            side,
            paths,
            title: title.into(),
            description: String::new(),
            primary: String::new(),
            secondary: String::new(),
            group: String::new(),
            options: TransferOptions::default(),
            algorithm: Algorithm::default(),
            manifest: ManifestMode::default(),
            numeric: String::new(),
            flags: [false; 4],
            criteria: MatchOptions::filename(""),
            pack: PackOptions::default(),
            extract: ExtractOptions::default(),
            archives: Vec::new(),
            archive_target: None,
            focus: kind != Kind::Unpack,
            focus_footer: matches!(
                kind,
                Kind::Delete | Kind::DeleteArchiveMembers | Kind::Unpack
            ),
            error: None,
            validating: None,
        }
    }
    pub fn transfer(side: Side, paths: Vec<PathBuf>, destination: PathBuf, moving: bool) -> Self {
        let title = if moving { "Move files" } else { "Copy files" };
        let mut dialog = Self::new(
            Kind::Transfer,
            if moving { Tool::Move } else { Tool::Copy },
            side,
            title,
            paths,
        );
        dialog.primary = destination.display().to_string();
        dialog.description = summarize(&dialog.paths);
        dialog
    }
    pub fn pack(side: Side, paths: Vec<PathBuf>, archive: &Path, folder: &Path) -> Self {
        let mut dialog = Self::new(Kind::Pack, Tool::Pack, side, "Add to archive", paths);
        dialog.primary = archive.display().to_string();
        dialog.description = summarize(&dialog.paths);
        dialog.archive_target = Some((archive.to_owned(), folder.to_owned()));
        dialog.pack.update = true;
        dialog
    }
    pub fn eject(path: PathBuf) -> Self {
        let mut dialog = Self::new(
            Kind::Eject,
            Tool::Settings,
            Side::Left,
            "Unmount volume",
            vec![path],
        );
        dialog.description = summarize(&dialog.paths);
        dialog
    }
}

fn menu_item(ui: &mut Ui, side: Side, text: &str, tool: Tool, actions: &mut Vec<Action>) {
    if ui.button(text).clicked() {
        actions.push(Action::Tool(side, tool));
        ui.close();
    }
}
fn cmd(ui: &mut Ui, text: &str, command: Command, actions: &mut Vec<Action>) {
    if ui.button(text).clicked() {
        actions.push(Action::Command(command));
        ui.close();
    }
}

pub(crate) fn file_menu(ui: &mut Ui, side: Side, actions: &mut Vec<Action>) {
    for (name, tool) in [
        ("New folder · F7", Tool::NewFolder),
        ("New text file · Shift+F4", Tool::NewFile),
        ("Go to folder…", Tool::FolderPicker),
        ("Rename · F2", Tool::Rename),
        ("Copy to other panel · F5", Tool::Copy),
        ("Copy in this folder · Shift+F5", Tool::CopyHere),
        ("Move to other panel · F6", Tool::Move),
    ] {
        menu_item(ui, side, name, tool, actions);
    }
    ui.separator();
    for (name, tool) in [
        ("Copy files", Tool::CopyClipboard),
        ("Cut files", Tool::CutClipboard),
        ("Paste files", Tool::Paste),
        ("Move to Trash · F8", Tool::Trash),
        ("Delete permanently…", Tool::Delete),
    ] {
        menu_item(ui, side, name, tool, actions);
    }
    ui.separator();
    menu_item(ui, side, "Pack…", Tool::Pack, actions);
    menu_item(ui, side, "Extract…", Tool::Unpack, actions);
    ui.separator();
    menu_item(ui, side, "Properties", Tool::Inspect, actions);
    if ui.button("Quit").clicked() {
        actions.push(Action::Close);
        ui.close();
    }
}
pub(crate) fn entry_menu(ui: &mut Ui, side: Side, entry: &Entry, actions: &mut Vec<Action>) {
    if entry.archive.is_some() {
        menu_item(ui, side, "Unlock archive…", Tool::UnlockArchive, actions);
        menu_item(ui, side, "Extract…", Tool::Unpack, actions);
        menu_item(ui, side, "Test archive…", Tool::TestArchive, actions);
        menu_item(
            ui,
            side,
            "Delete archive members…",
            Tool::DeleteArchiveMembers,
            actions,
        );
        return;
    }
    if !entry.is_dir() && engine::is_archive(&entry.path) {
        menu_item(ui, side, "Browse archive", Tool::BrowseArchive, actions);
        menu_item(
            ui,
            side,
            "Open with associated application",
            Tool::OpenAssociated,
            actions,
        );
        menu_item(ui, side, "Extract…", Tool::Unpack, actions);
        menu_item(ui, side, "Test archive…", Tool::TestArchive, actions);
    }
    let copy_path = format!(
        "Copy full path · {}",
        ui.ctx().format_shortcut(&COPY_PATH_SHORTCUT)
    );
    for (name, tool) in [
        ("Rename", Tool::Rename),
        ("Copy to other panel", Tool::Copy),
        ("Move to other panel", Tool::Move),
        (&copy_path, Tool::CopyPaths),
        ("Properties", Tool::Inspect),
        ("Move to Trash", Tool::Trash),
    ] {
        menu_item(ui, side, name, tool, actions);
    }
}
pub(crate) fn selection_menu(ui: &mut Ui, side: Side, actions: &mut Vec<Action>) {
    cmd(ui, "Mark all · Ctrl+A", Command::MarkAll(side), actions);
    cmd(ui, "Clear marks · Esc", Command::UnmarkAll(side), actions);
    cmd(ui, "Invert marks · Ctrl+I", Command::Invert(side), actions);
    menu_item(ui, side, "Mark by name or mask…", Tool::MarkMask, actions);
    menu_item(
        ui,
        side,
        "Unmark by name or mask…",
        Tool::UnmarkMask,
        actions,
    );
    ui.menu_button("Match focused item", |ui| {
        for (label, col) in [
            ("Base name", Column::Name),
            ("Extension", Column::Extension),
            ("Folder", Column::Path),
        ] {
            cmd(
                ui,
                &format!("Mark same {label}"),
                Command::MarkLike(side, col, true),
                actions,
            );
            cmd(
                ui,
                &format!("Unmark same {label}"),
                Command::MarkLike(side, col, false),
                actions,
            );
        }
    });
    ui.separator();
    cmd(ui, "Remember marks", Command::RememberMarks(side), actions);
    cmd(
        ui,
        "Restore remembered marks",
        Command::RestoreMarks(side),
        actions,
    );
    for (name, tool) in [
        ("Save marks to text file…", Tool::ExportMarks),
        ("Load marks from text file…", Tool::MarksFile),
        ("Load marks from clipboard", Tool::MarksClipboard),
        ("Open path list from text file…", Tool::ListFile),
        ("Open path list from clipboard", Tool::ListClipboard),
    ] {
        menu_item(ui, side, name, tool, actions);
    }
    ui.menu_button("Copy information", |ui| {
        let copy_path = format!(
            "Full paths · {}",
            ui.ctx().format_shortcut(&COPY_PATH_SHORTCUT)
        );
        for (name, tool) in [
            ("Names", Tool::CopyNames),
            (&copy_path, Tool::CopyPaths),
            ("Parent paths", Tool::CopyParentPaths),
            ("Displayed columns", Tool::CopyDetails),
            ("Export details…", Tool::ExportDetails),
        ] {
            menu_item(ui, side, name, tool, actions);
        }
    });
    ui.separator();
    menu_item(ui, side, "Quick Search…", Tool::Find, actions);
    menu_item(ui, side, "Filter… · Ctrl+F", Tool::FilterOptions, actions);
}
pub(crate) fn tools_menu(ui: &mut Ui, side: Side, actions: &mut Vec<Action>) {
    menu_item(ui, side, "Settings…", Tool::Settings, actions);
    ui.separator();
    menu_item(ui, side, "Open Terminal · F9", Tool::OpenTerminal, actions);
    ui.menu_button("Archives", |ui| {
        for (name, tool) in [
            ("Browse archive", Tool::BrowseArchive),
            ("Unlock archive…", Tool::UnlockArchive),
            ("Pack…", Tool::Pack),
            ("Extract…", Tool::Unpack),
            ("Delete archive members…", Tool::DeleteArchiveMembers),
            ("Test archive…", Tool::TestArchive),
        ] {
            menu_item(ui, side, name, tool, actions);
        }
    });
    #[cfg(target_os = "linux")]
    menu_item(
        ui,
        side,
        "Add DeshCmd to Application Menu",
        Tool::InstallMenuEntry,
        actions,
    );
    #[cfg(windows)]
    menu_item(
        ui,
        side,
        "Add DeshCmd to Start Menu",
        Tool::InstallMenuEntry,
        actions,
    );
    ui.menu_button("Workspace", |ui| {
        cmd(
            ui,
            "Open active path in both panels",
            Command::MatchPanes,
            actions,
        );
        menu_item(
            ui,
            side,
            "Open focused folder in other panel",
            Tool::FolderInOtherPane,
            actions,
        );
        menu_item(
            ui,
            side,
            "Save favorite workspace…",
            Tool::SaveFavorite,
            actions,
        );
        menu_item(ui, side, "Favorite workspaces…", Tool::Favorites, actions);
        menu_item(ui, side, "Save tab layout…", Tool::ExportTabs, actions);
        menu_item(ui, side, "Load tab layout…", Tool::ImportTabs, actions);
        menu_item(ui, side, "Add bookmark…", Tool::Bookmark, actions);
        menu_item(
            ui,
            side,
            "Export bookmarks…",
            Tool::ExportBookmarks,
            actions,
        );
        menu_item(
            ui,
            side,
            "Import bookmarks…",
            Tool::ImportBookmarks,
            actions,
        );
    });
    ui.menu_button("Create link", |ui| {
        menu_item(ui, side, "Symbolic link…", Tool::Symlink, actions);
        menu_item(ui, side, "Hard link…", Tool::Hardlink, actions);
        if cfg!(windows) {
            menu_item(ui, side, "Windows shortcut…", Tool::Shortcut, actions);
        }
    });
    for (name, tool) in [
        ("Calculate occupied space", Tool::Statistics),
        ("Change attributes…", Tool::ChangeAttributes),
        ("Edit comment…", Tool::Comment),
        ("Split file…", Tool::Split),
        ("Combine files…", Tool::Combine),
        ("Calculate checksums…", Tool::Hash),
        ("Verify checksums…", Tool::Verify),
        ("Hash benchmark", Tool::Benchmark),
        ("Wipe files…", Tool::Wipe),
    ] {
        menu_item(ui, side, name, tool, actions);
    }
}

fn details_text(
    tab: &engine::Tab,
    columns: &[Column],
    show_modified_time: bool,
    export: bool,
) -> String {
    fn format_rows<'a>(
        rows: impl Iterator<Item = &'a Entry>,
        columns: &[Column],
        show_modified_time: bool,
    ) -> String {
        let mut text = String::new();
        for (row, entry) in rows.enumerate() {
            if row != 0 {
                text.push('\n');
            }
            for (i, &column) in columns.iter().enumerate() {
                if i != 0 {
                    text.push('\t');
                }
                let value = crate::panes::value(entry, column, show_modified_time);
                text.extend(value.chars().map(|c| match c {
                    '\n' | '\r' | '\t' => ' ',
                    _ => c,
                }));
            }
        }
        text
    }
    if tab.marked_count() != 0 {
        format_rows(tab.marked_entries(), columns, show_modified_time)
    } else if export {
        format_rows(tab.visible_entries(), columns, show_modified_time)
    } else {
        format_rows(tab.current().into_iter(), columns, show_modified_time)
    }
}

impl App {
    pub(crate) fn invoke(&mut self, side: Side, tool: Tool, ctx: &egui::Context) {
        self.cancel_dialog_read();
        if matches!(tool, Tool::Settings) {
            self.open_settings(ctx);
            return;
        }
        let tab = self.engine.tab(side);
        if matches!(tool, Tool::ExportDetails)
            && tab.marked_count() == 0
            && tab.incomplete_listing()
        {
            self.notice("Wait for the directory to finish loading before exporting all details");
            return;
        }
        let base = tab.base().to_owned();
        let other_tab = self.engine.tab(side.other());
        let other = other_tab.base().to_owned();
        let archive = tab.archive_selection();
        let other_archive = other_tab.archive_selection();
        let missing = || "Select one file or folder to use this command";
        let empty = || "Select a file or folder first";
        if archive.is_some()
            && !matches!(
                tool,
                Tool::Copy
                    | Tool::Unpack
                    | Tool::TestArchive
                    | Tool::UnlockArchive
                    | Tool::DeleteArchiveMembers
                    | Tool::Delete
                    | Tool::Trash
                    | Tool::Find
                    | Tool::FilterOptions
                    | Tool::MarkMask
                    | Tool::UnmarkMask
                    | Tool::RenameTab(_)
                    | Tool::SaveFavorite
                    | Tool::Favorites
                    | Tool::ImportTabs
                    | Tool::ExportTabs
                    | Tool::FolderPicker
                    | Tool::CopyNames
                    | Tool::CopyPaths
                    | Tool::CopyParentPaths
                    | Tool::CopyDetails
                    | Tool::ExportDetails
                    | Tool::OpenTerminal
            )
        {
            self.notice("This action requires a local file or folder");
            return;
        }
        if matches!(tool, Tool::Move) && other_archive.is_some() {
            self.notice("Use Copy to add files to an archive");
            return;
        }
        let mut result = match tool {
            Tool::UnlockArchive => {
                if let Some(archive) = archive.as_ref() {
                    let mut dialog = Dialog::new(
                        Kind::UnlockArchive,
                        tool,
                        side,
                        "Unlock archive",
                        vec![archive.file.clone()],
                    );
                    dialog.description = archive.file.display().to_string();
                    Some(dialog)
                } else {
                    self.notice("Open the archive before unlocking it");
                    None
                }
            }
            Tool::BrowseArchive | Tool::OpenAssociated => {
                if let Some(entry) = tab
                    .single_operand()
                    .filter(|entry| !entry.is_dir() && engine::is_archive(&entry.path))
                {
                    let path = entry.path.clone();
                    if matches!(tool, Tool::BrowseArchive) {
                        self.apply(Action::Path(side, path), ctx);
                    } else if let Err(error) = self.desktop.open(&path) {
                        self.notice(error);
                    }
                } else {
                    self.notice("Select an archive first");
                }
                None
            }
            Tool::Pack if tab.has_operands() => {
                let paths = tab.operands();
                let name = paths
                    .first()
                    .and_then(|path| path.file_stem())
                    .unwrap_or_default()
                    .to_string_lossy();
                let mut dialog = Dialog::new(Kind::Pack, tool, side, "Pack files", paths.clone());
                dialog.primary = other.join(format!("{name}.zip")).display().to_string();
                dialog.description = summarize(&paths);
                Some(dialog)
            }
            Tool::Copy if archive.is_none() && other_archive.is_some() && tab.has_operands() => {
                let target = other_archive.as_ref().unwrap();
                Some(Dialog::pack(
                    side,
                    tab.operands(),
                    &target.file,
                    &target.folder,
                ))
            }
            Tool::Unpack | Tool::Copy if matches!(tool, Tool::Unpack) || archive.is_some() => {
                if matches!(tool, Tool::Copy) && other_archive.is_some() {
                    self.notice("Choose a local destination for the extracted files");
                    return;
                }
                let archives = if let Some(selection) = archive {
                    if matches!(tool, Tool::Copy) && selection.members.is_empty() {
                        self.notice("Select archive members to copy");
                        return;
                    }
                    vec![selection]
                } else {
                    tab.operands()
                        .into_iter()
                        .filter(|path| path.is_file() && engine::is_archive(path))
                        .map(|file| ArchiveSelection {
                            file,
                            folder: PathBuf::new(),
                            members: Vec::new(),
                        })
                        .collect()
                };
                if archives.is_empty() {
                    self.notice("Select an archive first");
                    return;
                }
                let mut dialog = Dialog::new(
                    Kind::Unpack,
                    tool,
                    side,
                    "Extract archive contents",
                    Vec::new(),
                );
                dialog.primary = other.display().to_string();
                dialog.description = summarize(
                    &archives
                        .iter()
                        .map(|source| source.file.clone())
                        .collect::<Vec<_>>(),
                );
                dialog.archives = archives;
                if let [source] = dialog.archives.as_slice()
                    && let Some(password) = self.engine.archive_password(&source.file)
                {
                    dialog.extract.password = password.to_string();
                }
                Some(dialog)
            }
            Tool::DeleteArchiveMembers | Tool::Delete | Tool::Trash if archive.is_some() => {
                let selection = archive.unwrap();
                if selection.members.is_empty() {
                    self.notice("Select archive members to delete");
                    return;
                }
                let mut dialog = Dialog::new(
                    Kind::DeleteArchiveMembers,
                    tool,
                    side,
                    "Delete archive members",
                    Vec::new(),
                );
                dialog.description = summarize(&selection.members);
                dialog.archives = vec![selection];
                Some(dialog)
            }
            Tool::TestArchive => {
                let paths = if let Some(archive) = archive {
                    vec![archive.file]
                } else {
                    tab.operands()
                        .into_iter()
                        .filter(|path| path.is_file() && engine::is_archive(path))
                        .collect()
                };
                if paths.is_empty() {
                    self.notice("Select an archive first");
                    return;
                }
                let mut dialog = Dialog::new(
                    Kind::TestArchive,
                    tool,
                    side,
                    "Test archive integrity",
                    paths,
                );
                dialog.description = summarize(&dialog.paths);
                if let [path] = dialog.paths.as_slice()
                    && let Some(password) = self.engine.archive_password(path)
                {
                    dialog.extract.password = password.to_string();
                }
                Some(dialog)
            }
            Tool::Pack | Tool::DeleteArchiveMembers | Tool::Unpack => {
                self.notice(if matches!(tool, Tool::Pack) {
                    empty()
                } else if matches!(tool, Tool::Unpack) {
                    "Select an archive first"
                } else {
                    "Open an archive to delete its members"
                });
                None
            }
            Tool::OpenTerminal => {
                if let Err(error) = self.desktop.open_terminal(&base) {
                    self.notice(format!(
                        "Could not open terminal in {}. {error}",
                        base.display()
                    ));
                }
                None
            }
            #[cfg(any(target_os = "linux", windows))]
            Tool::InstallMenuEntry => {
                let menu = if cfg!(windows) {
                    "Start menu"
                } else {
                    "application menu"
                };
                let message = match self.desktop.install_menu_entry() {
                    Ok(()) => format!("Added DeshCmd to the {menu}"),
                    Err(error) => format!("Could not add DeshCmd to the {menu}. {error}"),
                };
                self.notice(message);
                None
            }
            Tool::Copy | Tool::CopyHere | Tool::Move if tab.has_operands() => {
                Some(Dialog::transfer(
                    side,
                    tab.operands(),
                    if matches!(tool, Tool::CopyHere) {
                        base
                    } else {
                        other
                    },
                    matches!(tool, Tool::Move),
                ))
            }
            Tool::Copy | Tool::CopyHere | Tool::Move => {
                self.notice(empty());
                None
            }
            Tool::NewFolder
            | Tool::NewFile
            | Tool::Rename
            | Tool::Symlink
            | Tool::Hardlink
            | Tool::Shortcut
            | Tool::RenameTab(_)
            | Tool::SaveFavorite
            | Tool::Comment => {
                let title = match tool {
                    Tool::NewFolder => "New folder",
                    Tool::NewFile => "New text file",
                    Tool::Rename => "Rename item",
                    Tool::Symlink => "Create symbolic link",
                    Tool::Hardlink => "Create hard link",
                    Tool::Shortcut => "Create Windows shortcut",
                    Tool::RenameTab(_) => "Rename tab",
                    Tool::SaveFavorite => "Save favorite workspace",
                    Tool::Comment => "Edit file comment",
                    _ => unreachable!(),
                };
                let candidate = if matches!(
                    tool,
                    Tool::Rename | Tool::Symlink | Tool::Hardlink | Tool::Shortcut | Tool::Comment
                ) {
                    let Some(entry) = tab.single_operand() else {
                        self.notice(missing());
                        return;
                    };
                    Some(entry)
                } else {
                    None
                };
                let paths = candidate
                    .into_iter()
                    .map(|entry| entry.path.clone())
                    .collect();
                let mut dialog = Dialog::new(Kind::Text, tool, side, title, paths);
                match tool {
                    Tool::NewFolder => dialog.description = base.display().to_string(),
                    Tool::NewFile => {
                        dialog.primary = default_text_filename(&base);
                        dialog.description = base.display().to_string();
                    }
                    Tool::Rename => {
                        dialog.primary = candidate
                            .as_ref()
                            .map(|item| item.name.clone())
                            .unwrap_or_default()
                    }
                    Tool::Symlink | Tool::Hardlink | Tool::Shortcut => {
                        dialog.primary = other
                            .join(
                                candidate
                                    .as_ref()
                                    .map(|entry| entry.name.as_str())
                                    .unwrap_or("link"),
                            )
                            .display()
                            .to_string();
                        if matches!(tool, Tool::Shortcut) {
                            dialog.primary.push_str(".lnk");
                        }
                        dialog.flags[0] = true;
                    }
                    Tool::RenameTab(index) => {
                        let Some(tab) = self.engine.pane(side).tabs.get(index) else {
                            return;
                        };
                        dialog.primary = tab.title();
                    }
                    Tool::Comment => {
                        dialog.primary = candidate
                            .as_ref()
                            .map(|item| item.comment.clone())
                            .unwrap_or_default()
                    }
                    _ => {}
                }
                Some(dialog)
            }
            Tool::Trash | Tool::Delete | Tool::Wipe if tab.has_operands() => {
                let title = match tool {
                    Tool::Trash => "Move to Trash",
                    Tool::Delete => "Delete permanently",
                    _ => "Overwrite and delete",
                };
                let mut dialog = Dialog::new(
                    if matches!(tool, Tool::Wipe) {
                        Kind::Wipe
                    } else {
                        Kind::Delete
                    },
                    tool,
                    side,
                    title,
                    tab.operands(),
                );
                dialog.description = summarize(&dialog.paths);
                dialog.numeric = "3".into();
                let confirm = match tool {
                    Tool::Trash => self.engine.settings.confirm_trash,
                    Tool::Delete => self.engine.settings.confirm_delete,
                    _ => true,
                };
                if confirm {
                    Some(dialog)
                } else {
                    if let Err(error) = self.submit(&dialog, false, ctx) {
                        self.notice(error);
                    }
                    None
                }
            }
            Tool::Trash | Tool::Delete | Tool::Wipe => {
                self.notice(empty());
                None
            }
            Tool::Inspect => {
                if let Some(entry) = tab.single_operand().or_else(|| tab.current()) {
                    let mut dialog = Dialog::new(
                        Kind::Inspect,
                        tool,
                        side,
                        "File properties",
                        vec![entry.path.clone()],
                    );
                    dialog.description = describe(entry);
                    if entry.archive.is_none() {
                        let path = entry.path.clone();
                        self.read_for_tab(
                            side,
                            move || engine::local_properties(&path),
                            move |app, _, extra| {
                                if let (Some(dialog), Some(extra)) = (&mut app.dialog, extra)
                                    && dialog.kind == Kind::Inspect
                                    && dialog.side == side
                                {
                                    dialog.description.push('\n');
                                    dialog.description.push_str(&extra);
                                }
                            },
                        );
                    }
                    Some(dialog)
                } else {
                    self.notice(empty());
                    None
                }
            }
            Tool::ChangeAttributes if tab.has_operands() => {
                let mut dialog = Dialog::new(
                    Kind::Attributes,
                    tool,
                    side,
                    "Change attributes",
                    tab.operands(),
                );
                dialog.description = summarize(&dialog.paths);
                dialog.primary = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                Some(dialog)
            }
            Tool::ChangeAttributes => {
                self.notice(empty());
                None
            }
            Tool::Statistics if tab.has_operands() => {
                self.apply(
                    Action::Command(Command::Queue(Operation::Statistics(tab.operands()), false)),
                    ctx,
                );
                None
            }
            Tool::Statistics => {
                self.notice(empty());
                None
            }
            Tool::Split if tab.single_operand().is_some_and(|entry| !entry.is_dir()) => {
                let mut dialog = Dialog::new(Kind::Split, tool, side, "Split file", tab.operands());
                dialog.primary = other.display().to_string();
                dialog.numeric = "64".into();
                dialog.description = summarize(&dialog.paths);
                Some(dialog)
            }
            Tool::Split => {
                self.notice("Select one regular file to split");
                None
            }
            Tool::Combine if tab.has_operands() => {
                if let Some(entry) = tab.single_operand() {
                    let path = entry.path.clone();
                    self.read_for_tab(
                        side,
                        move || engine::numbered_parts(&path),
                        move |app, _, parts| {
                            if app.dialog.is_none() {
                                app.dialog = Some(combine_dialog(side, &other, parts));
                            }
                        },
                    );
                    None
                } else {
                    Some(combine_dialog(side, &other, tab.operands()))
                }
            }
            Tool::Combine => {
                self.notice("Select the numbered parts or their first file");
                None
            }
            Tool::Hash if tab.has_operands() => {
                let mut dialog = Dialog::new(
                    Kind::Hash,
                    tool,
                    side,
                    "Calculate checksums",
                    tab.operands(),
                );
                dialog.primary = base.join("checksums.blake3").display().to_string();
                dialog.description = summarize(&dialog.paths);
                Some(dialog)
            }
            Tool::Hash => {
                self.notice(empty());
                None
            }
            Tool::Verify if tab.single_operand().is_some_and(|entry| !entry.is_dir()) => {
                let mut dialog =
                    Dialog::new(Kind::Verify, tool, side, "Verify checksums", tab.operands());
                let algorithm = Algorithm::from_path(&dialog.paths[0]).or_else(|| {
                    dialog.paths[0]
                        .extension()
                        .filter(|extension| extension.eq_ignore_ascii_case("sfv"))
                        .map(|_| Algorithm::Crc32)
                });
                dialog.algorithm = algorithm.unwrap_or_default();
                dialog.flags[0] = algorithm.is_some();
                dialog.description = summarize(&dialog.paths);
                Some(dialog)
            }
            Tool::Verify => {
                self.notice("Select one file or checksum manifest");
                None
            }
            Tool::Benchmark => {
                self.apply(
                    Action::Command(Command::Queue(Operation::Benchmark, false)),
                    ctx,
                );
                None
            }
            Tool::CopyClipboard | Tool::CutClipboard => {
                let selected = tab.operands();
                if selected.is_empty() {
                    self.notice(empty());
                    return;
                }
                let cutting = matches!(tool, Tool::CutClipboard);
                let published = match self.desktop.copy_files(&selected) {
                    Ok(()) => {
                        self.notice(if cutting {
                            "Cut files to clipboard"
                        } else {
                            "Copied files to clipboard"
                        });
                        true
                    }
                    Err(error) => {
                        self.notice(format!(
                            "Files are available for paste inside deshcmd · {error}"
                        ));
                        false
                    }
                };
                self.interface.clipboard = Some(FileClipboard {
                    paths: selected,
                    cutting,
                    published,
                });
                None
            }
            Tool::Paste => {
                let clipboard = self.desktop.clipboard_files();
                let (paths, moving) = match &self.interface.clipboard {
                    Some(local)
                        if local.paths == clipboard
                            || (!local.published
                                && clipboard.is_empty()
                                && self.desktop.clipboard_text().is_none()) =>
                    {
                        (local.paths.clone(), local.cutting)
                    }
                    _ => (clipboard, false),
                };
                if paths.is_empty() {
                    self.notice("Clipboard does not contain files");
                    None
                } else {
                    Some(Dialog::transfer(side, paths, base, moving))
                }
            }
            Tool::CopyNames
            | Tool::CopyPaths
            | Tool::CopyParentPaths
            | Tool::CopyDetails
            | Tool::ExportDetails
            | Tool::ExportMarks => {
                let text = if matches!(tool, Tool::ExportMarks) {
                    tab.marked_entries()
                        .map(|entry| entry.path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                } else if matches!(tool, Tool::CopyDetails | Tool::ExportDetails) {
                    let columns = if self.interface.alternate_columns[side.index()] {
                        &self.engine.settings.alternate_columns
                    } else {
                        &self.engine.settings.columns
                    };
                    details_text(
                        tab,
                        columns,
                        self.engine.settings.show_modified_time,
                        matches!(tool, Tool::ExportDetails),
                    )
                } else {
                    tab.operands()
                        .iter()
                        .map(|path| match tool {
                            Tool::CopyNames => engine::display_name(path),
                            Tool::CopyParentPaths => {
                                path.parent().unwrap_or(path).display().to_string()
                            }
                            _ => path.display().to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if text.is_empty() {
                    self.notice("There are no items to export");
                } else if matches!(tool, Tool::ExportDetails | Tool::ExportMarks) {
                    let name = if matches!(tool, Tool::ExportMarks) {
                        "selection.txt"
                    } else {
                        "files.tsv"
                    };
                    if let Some(output) = self.desktop.save_file(&base, name, "Export file list") {
                        self.apply(
                            Action::Command(Command::Queue(
                                Operation::Export {
                                    output,
                                    contents: text,
                                },
                                false,
                            )),
                            ctx,
                        );
                    }
                } else {
                    self.interface.clipboard = None;
                    self.apply(Action::ClipboardText(text), ctx);
                }
                None
            }
            Tool::MarksFile | Tool::ListFile => {
                if let Some(path) = self.desktop.pick_file(&base, "Open text file with paths") {
                    self.read_for_tab(
                        side,
                        move || engine::read_list(&path),
                        move |app, _, result| match result {
                            Ok(text) => app.engine.dispatch(if matches!(tool, Tool::MarksFile) {
                                Command::LoadMarks(side, text)
                            } else {
                                Command::LoadList(side, text)
                            }),
                            Err(error) => app.notice(error.to_string()),
                        },
                    );
                }
                None
            }
            Tool::MarksClipboard | Tool::ListClipboard => {
                if let Some(text) = self.desktop.clipboard_text() {
                    self.engine
                        .dispatch(if matches!(tool, Tool::MarksClipboard) {
                            Command::LoadMarks(side, text)
                        } else {
                            Command::LoadList(side, text)
                        });
                } else {
                    self.notice("Clipboard has no text");
                }
                None
            }
            Tool::Find | Tool::MarkMask | Tool::UnmarkMask | Tool::FilterOptions => {
                let title = match tool {
                    Tool::Find => "Quick Search",
                    Tool::MarkMask => "Mark by mask",
                    Tool::UnmarkMask => "Unmark by mask",
                    _ => "Quick Filter",
                };
                let mut dialog = Dialog::new(Kind::Criteria, tool, side, title, Vec::new());
                if matches!(tool, Tool::FilterOptions) {
                    dialog.criteria = tab.filter().clone();
                    dialog.flags[0] = matches!(
                        tab.location(),
                        engine::Location::Directory(_) | engine::Location::Archive { .. }
                    );
                }
                Some(dialog)
            }
            Tool::Bookmark | Tool::EditBookmark(_) => {
                let mut dialog = Dialog::new(
                    Kind::Bookmark,
                    tool,
                    side,
                    "Bookmark folder",
                    vec![base.clone()],
                );
                dialog.primary = engine::display_name(&base);
                dialog.secondary = base.display().to_string();
                dialog.description = other.display().to_string();
                if let Tool::EditBookmark(index) = tool
                    && let Some(bookmark) = self.engine.settings.bookmarks.get(index)
                {
                    dialog.primary.clone_from(&bookmark.name);
                    dialog.secondary = bookmark.path.display().to_string();
                    dialog.group.clone_from(&bookmark.group);
                    if let Some(path) = &bookmark.opposite {
                        dialog.description = path.display().to_string();
                        dialog.flags[0] = true;
                    }
                }
                Some(dialog)
            }
            Tool::FolderInOtherPane => {
                if let Some(entry) = tab.current().filter(|entry| entry.is_dir()) {
                    self.engine.dispatch(Command::Navigate {
                        side: side.other(),
                        path: entry.path.clone(),
                        new_tab: false,
                        background: false,
                    });
                    self.engine.dispatch(Command::Focus(side));
                } else {
                    self.notice("Focus a folder first");
                }
                None
            }
            Tool::FolderPicker => {
                if let Some(path) = self.desktop.pick_directory(&base, "Open folder") {
                    self.apply(Action::Path(side, path), ctx);
                }
                None
            }
            Tool::Favorites => Some(Dialog::new(
                Kind::Favorites,
                tool,
                side,
                "Favorite workspaces",
                Vec::new(),
            )),
            Tool::ExportTabs | Tool::ExportBookmarks => {
                let state = self.engine.snapshot();
                let (name, json) = if matches!(tool, Tool::ExportTabs) {
                    (
                        "workspace.deshcmd-tabs",
                        serde_json::to_string_pretty(&state.panes),
                    )
                } else {
                    (
                        "bookmarks.deshcmd",
                        serde_json::to_string_pretty(&state.bookmarks),
                    )
                };
                if let Some(output) = self.desktop.save_file(&base, name, "Export workspace data") {
                    match json {
                        Ok(contents) => self.apply(
                            Action::Command(Command::Queue(
                                Operation::Export { output, contents },
                                false,
                            )),
                            ctx,
                        ),
                        Err(error) => self.notice(error.to_string()),
                    }
                }
                None
            }
            Tool::ImportTabs | Tool::ImportBookmarks => {
                if let Some(file) = self.desktop.pick_file(&base, "Import workspace data") {
                    self.read_for_tab(
                        side,
                        move || {
                            engine::read_list(&file).and_then(|text| {
                                Ok(if matches!(tool, Tool::ImportTabs) {
                                    Command::RestoreTabs(serde_json::from_str(&text)?)
                                } else {
                                    Command::ImportBookmarks(serde_json::from_str(&text)?)
                                })
                            })
                        },
                        |app, _, result| match result {
                            Ok(command) => app.engine.dispatch(command),
                            Err(error) => app.notice(error.to_string()),
                        },
                    );
                }
                None
            }
            Tool::Settings => unreachable!(),
            Tool::JobDetails(id) => {
                let Some(job) = self.engine.jobs.iter().find(|job| job.id == id) else {
                    return;
                };
                let mut dialog = Dialog::new(Kind::HelpDetail, tool, side, job.title, Vec::new());
                dialog.description = std::iter::once(job.message.as_str())
                    .chain(job.warnings.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(dialog)
            }
        };
        if let Some(result) = result.take() {
            self.dialog = Some(result);
        }
    }

    pub(crate) fn dialogs(&mut self, ctx: &egui::Context) {
        if self.dialog.is_none() {
            self.conflicts(ctx);
            return;
        }
        let mut dialog = self.dialog.take().unwrap();
        if dialog
            .validating
            .is_some_and(|origin| origin != self.engine.tab(dialog.side).navigation_stamp())
        {
            self.cancel_dialog_read();
            return;
        }
        let validating = dialog.validating.is_some();
        let mut outcome = Outcome::None;
        let title = dialog.title.clone();
        let response = egui::Modal::new(Id::unique("deshcmd-operation-modal")).show(ctx, |ui| {
            ui.set_width(475.);
            ui.horizontal(|ui| {
                ui.label(label(&title, 21., TEXT).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Close, "Close dialog", true).clicked() {
                        outcome = Outcome::Close;
                    }
                });
            });
            ui.add_space(6.);
            if dialog.kind == Kind::Favorites {
                draw_favorites(
                    ui,
                    &mut dialog,
                    &self.engine.settings.favorites,
                    &mut outcome,
                );
            } else {
                ui.add_enabled_ui(!validating, |ui| draw_fields(ui, &mut dialog, &mut outcome));
                if validating {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(label("Checking destination…", 12., SUB));
                    });
                }
            }
            if let Some(error) = dialog.error.as_deref() {
                ui.label(label(error, 12., RED));
            }
            ui.add_space(7.);
            ui.separator();
            ui.horizontal(|ui| {
                if matches!(dialog.tool, Tool::FilterOptions) && ui.button("Clear filter").clicked()
                {
                    dialog.criteria = MatchOptions::filename("");
                    outcome = Outcome::Submit(false);
                }
                if matches!(dialog.kind, Kind::Transfer | Kind::Pack | Kind::Unpack)
                    && ui
                        .button("Queue paused")
                        .on_hover_text("Add this operation and start it later from the queue")
                        .clicked()
                {
                    outcome = Outcome::Submit(true);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let only_close = matches!(
                        dialog.kind,
                        Kind::Inspect | Kind::Favorites | Kind::HelpDetail
                    );
                    let button_text = match dialog.kind {
                        Kind::Delete if matches!(dialog.tool, Tool::Trash) => "Move to Trash",
                        Kind::Delete => "Delete permanently",
                        Kind::Pack => "Pack",
                        Kind::Unpack => "Extract",
                        Kind::DeleteArchiveMembers => "Delete members",
                        Kind::TestArchive => "Test",
                        Kind::UnlockArchive => "Unlock",
                        Kind::Wipe => "Overwrite and delete",
                        Kind::Transfer if matches!(dialog.tool, Tool::Move) => "Move",
                        Kind::Transfer => "Copy",
                        Kind::Eject => "Unmount",
                        Kind::Bookmark => "Save",
                        _ if only_close => "Close",
                        _ => "Apply",
                    };
                    let dangerous = matches!(
                        dialog.kind,
                        Kind::Delete | Kind::Wipe | Kind::DeleteArchiveMembers
                    );
                    let ink = BASE;
                    let fill = if dangerous { RED } else { ACCENT };
                    let confirm = ui.add(
                        egui::Button::new(label(button_text, 13., ink).strong())
                            .fill(fill)
                            .stroke(egui::Stroke::NONE),
                    );
                    if confirm.clicked() {
                        outcome = if only_close {
                            Outcome::Close
                        } else {
                            Outcome::Submit(false)
                        };
                    }
                    if !only_close {
                        let cancel = ui.button("Cancel");
                        if std::mem::take(&mut dialog.focus_footer) {
                            if dangerous {
                                cancel.request_focus();
                            } else {
                                confirm.request_focus();
                            }
                        }
                        if matches!(
                            dialog.kind,
                            Kind::Delete | Kind::DeleteArchiveMembers | Kind::Unpack
                        ) {
                            for button in [&confirm, &cancel] {
                                if button.has_focus() {
                                    ui.painter().rect_stroke(
                                        button.rect.expand(2.),
                                        5.,
                                        egui::Stroke::new(1.5, TEXT),
                                        egui::StrokeKind::Outside,
                                    );
                                }
                            }
                        }
                        if cancel.clicked() {
                            outcome = Outcome::Close;
                        }
                    }
                });
            });
        });
        if response.should_close() && matches!(outcome, Outcome::None) {
            outcome = Outcome::Close;
        }
        if validating && !matches!(outcome, Outcome::Close) {
            self.dialog = Some(dialog);
            return;
        }
        match outcome {
            Outcome::None => self.dialog = Some(dialog),
            Outcome::Close => self.cancel_dialog_read(),
            Outcome::Browse => {
                let initial = self.engine.tab(dialog.side).base();
                if let Some(path) = self.desktop.pick_directory(initial, "Choose destination") {
                    dialog.primary = path.display().to_string();
                }
                self.dialog = Some(dialog);
            }
            Outcome::Favorite(index, remove) => {
                if remove {
                    self.engine.dispatch(Command::RemoveFavorite(index));
                    self.dialog = Some(dialog);
                } else {
                    self.engine
                        .dispatch(Command::LoadFavorite(index, dialog.flags[0]));
                }
            }
            Outcome::Submit(held) => {
                match self.probe_dialog_destination(&mut dialog, held) {
                    Ok(true) => {
                        self.dialog = Some(dialog);
                        return;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        dialog.error = Some(error);
                        self.dialog = Some(dialog);
                        return;
                    }
                }
                if let Err(error) = self.submit(&dialog, held, ctx) {
                    dialog.error = Some(error);
                    self.dialog = Some(dialog);
                } else if dialog.kind == Kind::Delete && dialog.flags[0] {
                    if matches!(dialog.tool, Tool::Trash) {
                        self.engine.settings.confirm_trash = false;
                    } else {
                        self.engine.settings.confirm_delete = false;
                    }
                }
            }
        }
    }

    fn conflicts(&mut self, ctx: &egui::Context) {
        let Some(job) = self.engine.jobs.iter().find(|job| job.conflict.is_some()) else {
            return;
        };
        let Some(conflict) = job.conflict.clone() else {
            return;
        };
        let id = job.id;
        let comparison = job.message.clone();
        let mut compare = false;
        let checkbox = Id::unique(("conflict-apply-all", id));
        let mut all = ctx.data(|data| data.get_temp(checkbox).unwrap_or(false));
        let mut choice = None;
        let response = egui::Modal::new(Id::unique(("transfer-conflict", id))).show(ctx, |ui| {
            ui.set_width(475.);
            ui.label(label("A name is already in use", 21., TEXT).strong());
            ui.add_space(8.);
            ui.label(label("Incoming", 11., MUTED).strong());
            ui.label(mono(conflict.source.display().to_string(), 12., TEXT));
            ui.label(label("Destination", 11., MUTED).strong());
            ui.label(mono(conflict.target.display().to_string(), 12., TEXT));
            ui.add_space(6.);
            let mut choices = vec![
                ("Skip", Collision::Skip),
                ("Rename incoming", Collision::RenameIncoming),
                ("Rename existing", Collision::RenameExisting),
            ];
            if conflict.directory {
                choices.push(("Merge folders", Collision::Merge));
            } else {
                choices.extend([
                    ("Replace", Collision::Replace),
                    ("Replace if newer", Collision::Newer),
                    ("Replace if larger", Collision::Larger),
                    ("Replace if smaller", Collision::Smaller),
                ]);
                if conflict.can_append {
                    choices.extend([
                        ("Append", Collision::Append),
                        ("Resume partial file", Collision::Resume),
                    ]);
                }
            }
            ui.checkbox(
                &mut all,
                "Use this choice for the remaining conflicts of this type",
            );
            ui.horizontal_wrapped(|ui| {
                for (caption, value) in choices {
                    if ui.button(caption).clicked() {
                        choice = Some(value);
                    }
                }
            });
            if conflict.can_append {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(comparison.is_empty(), egui::Button::new("Compare contents"))
                        .clicked()
                    {
                        compare = true;
                    }
                    if !comparison.is_empty() {
                        ui.label(label(&comparison, 12., SUB));
                    }
                });
            }
            ui.separator();
            if ui.button("Cancel operation").clicked() {
                choice = Some(Collision::Cancel);
            }
        });
        if compare {
            self.engine.dispatch(Command::CompareConflict(id));
        }
        if response.should_close() {
            choice = Some(Collision::Cancel);
        }
        if let Some(choice) = choice {
            ctx.data_mut(|data| data.remove::<bool>(checkbox));
            self.engine.dispatch(Command::Resolve(id, choice, all));
        } else {
            ctx.data_mut(|data| data.insert_temp(checkbox, all));
        }
    }

    fn probe_dialog_destination(
        &mut self,
        dialog: &mut Dialog,
        held: bool,
    ) -> Result<bool, String> {
        let side = dialog.side;
        let tab = self.engine.tab(side);
        let path = |text: &str| absolute_input(tab.base(), &self.engine.home, text);
        let (paths, message) = match dialog.kind {
            Kind::Transfer | Kind::Split => (
                vec![path(&dialog.primary)?],
                "Choose an existing destination folder",
            ),
            Kind::Bookmark => {
                if dialog.primary.trim().is_empty() {
                    return Err("Enter a bookmark name".into());
                }
                let mut paths = vec![path(&dialog.secondary)?];
                if dialog.flags[0] {
                    paths.push(path(&dialog.description)?);
                }
                (paths, "Choose existing bookmark folders")
            }
            _ => return Ok(false),
        };
        dialog.validating = Some(tab.navigation_stamp());
        dialog.error = None;
        self.read_for_tab(
            side,
            move || paths.iter().all(|path| path.is_dir()),
            move |app, ctx, valid| {
                let Some(mut dialog) = app.dialog.take() else {
                    return;
                };
                if dialog.validating.take().is_none() {
                    app.dialog = Some(dialog);
                    return;
                }
                let result = if valid {
                    app.submit(&dialog, held, ctx)
                } else {
                    Err(message.into())
                };
                if let Err(error) = result {
                    dialog.error = Some(error);
                    app.dialog = Some(dialog);
                }
            },
        );
        Ok(true)
    }

    fn submit(&mut self, dialog: &Dialog, held: bool, ctx: &egui::Context) -> Result<(), String> {
        let Dialog {
            kind,
            tool,
            side,
            paths,
            primary,
            secondary,
            group,
            options,
            algorithm,
            manifest,
            numeric,
            flags,
            criteria,
            ..
        } = dialog;
        let side = *side;
        let base = self.engine.tab(side).base();
        let path_input = |text: &str| absolute_input(base, &self.engine.home, text);
        let item = || {
            paths
                .first()
                .cloned()
                .ok_or_else(|| "Choose an item first".to_string())
        };
        let queue = |operation| Action::Command(Command::Queue(operation, held));
        let action = match kind {
            Kind::UnlockArchive => Action::Command(Command::UnlockArchive {
                side,
                file: item()?,
                password: dialog.extract.password.clone(),
            }),
            Kind::Pack => {
                if primary.trim().is_empty() {
                    return Err("Enter the archive path".into());
                }
                let (output, folder) = match &dialog.archive_target {
                    Some(target) => target.clone(),
                    None => (path_input(primary)?, PathBuf::new()),
                };
                queue(Operation::Archive(ArchiveOperation::Pack {
                    paths: paths.clone(),
                    output,
                    folder,
                    options: dialog.pack.clone(),
                }))
            }
            Kind::Unpack => {
                if primary.trim().is_empty() {
                    return Err("Enter a destination folder".into());
                }
                queue(Operation::Archive(ArchiveOperation::Extract {
                    sources: dialog.archives.clone(),
                    destination: path_input(primary)?,
                    options: dialog.extract.clone(),
                }))
            }
            Kind::DeleteArchiveMembers => queue(Operation::Archive(ArchiveOperation::Delete(
                dialog.archives[0].clone(),
            ))),
            Kind::TestArchive => queue(Operation::Archive(ArchiveOperation::Test {
                paths: paths.clone(),
                password: dialog.extract.password.clone(),
            })),
            Kind::Text => match tool {
                Tool::NewFolder => {
                    let relative = Path::new(primary.trim());
                    if relative.as_os_str().is_empty()
                        || relative
                            .components()
                            .any(|component| !matches!(component, Component::Normal(_)))
                    {
                        return Err("Enter a folder name or a relative path of new folders".into());
                    }
                    queue(Operation::Mkdir(base.join(relative)))
                }
                Tool::NewFile => Action::Command(Command::CreateFile {
                    side,
                    path: base.join(filename(primary)?),
                    hold: held,
                }),
                Tool::Rename => {
                    let source = item()?;
                    let target = source.with_file_name(filename(primary)?);
                    queue(Operation::Rename { source, target })
                }
                Tool::Symlink | Tool::Hardlink => queue(Operation::Link {
                    source: item()?,
                    target: path_input(primary)?,
                    symbolic: matches!(tool, Tool::Symlink),
                    relative: flags[0],
                }),
                Tool::Shortcut => {
                    let source = item()?;
                    let target = path_input(primary)?;
                    self.desktop.shortcut(&target, &source)?;
                    self.engine.dispatch(Command::Refresh);
                    self.notice("Created Windows shortcut");
                    return Ok(());
                }
                Tool::RenameTab(index) => {
                    Action::Command(Command::RenameTab(side, *index, primary.clone()))
                }
                Tool::SaveFavorite => {
                    if primary.trim().is_empty() {
                        return Err("Enter a name for this workspace".into());
                    }
                    Action::Command(Command::SaveFavorite(primary.trim().to_owned()))
                }
                Tool::Comment => queue(Operation::Comment {
                    path: item()?,
                    text: primary.clone(),
                }),
                _ => unreachable!(),
            },
            Kind::Transfer => {
                let destination = path_input(primary)?;
                queue(Operation::Transfer {
                    paths: paths.clone(),
                    destination,
                    moving: matches!(tool, Tool::Move),
                    options: options.clone(),
                })
            }
            Kind::Delete | Kind::Wipe => {
                if paths.iter().any(|path| path.parent().is_none()) {
                    return Err("A filesystem root cannot be deleted".into());
                }
                if *kind == Kind::Wipe {
                    let passes = numeric
                        .parse::<u8>()
                        .ok()
                        .filter(|count| (1..=16).contains(count))
                        .ok_or("Enter 1 to 16 overwrite passes")?;
                    queue(Operation::Wipe {
                        paths: paths.clone(),
                        passes,
                    })
                } else {
                    queue(Operation::Delete {
                        paths: paths.clone(),
                        trash: matches!(tool, Tool::Trash),
                    })
                }
            }
            Kind::Attributes => {
                let modified = if flags[2] {
                    let date =
                        chrono::NaiveDateTime::parse_from_str(primary.trim(), "%Y-%m-%d %H:%M:%S")
                            .map_err(
                                |_| "Enter a valid local date and time as YYYY-MM-DD HH:MM:SS",
                            )?;
                    let local = date
                        .and_local_timezone(chrono::Local)
                        .single()
                        .ok_or("This local time is ambiguous or unavailable")?;
                    Some(local.into())
                } else {
                    None
                };
                if !flags[0] && modified.is_none() {
                    return Err("Choose an attribute or timestamp to change".into());
                }
                queue(Operation::Properties {
                    paths: paths.clone(),
                    read_only: flags[0].then_some(flags[1]),
                    modified,
                    recursive: flags[3],
                })
            }
            Kind::Split => {
                let quantity = numeric
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or("Enter a positive part size or count")?;
                let destination = path_input(primary)?;
                queue(Operation::Split {
                    source: item()?,
                    destination,
                    part_bytes: quantity.saturating_mul(1024 * 1024),
                    part_count: flags[0].then_some(quantity),
                })
            }
            Kind::Combine => queue(Operation::Combine {
                paths: paths.clone(),
                output: path_input(primary)?,
            }),
            Kind::Hash => queue(Operation::Hash {
                paths: paths.clone(),
                algorithm: *algorithm,
                mode: *manifest,
                output: if *manifest == ManifestMode::Single {
                    path_input(primary)?
                } else {
                    base.join("checksums")
                },
            }),
            Kind::Verify => {
                if !flags[0] && primary.trim().is_empty() {
                    return Err("Paste the expected checksum or switch to manifest mode".into());
                }
                queue(Operation::Verify {
                    source: item()?,
                    expected: if flags[0] {
                        None
                    } else {
                        Some(primary.trim().to_owned())
                    },
                    algorithm: *algorithm,
                })
            }
            Kind::Criteria => {
                let command = match tool {
                    Tool::Find => Command::Find(side, criteria.clone()),
                    Tool::MarkMask => Command::MarkMask(side, criteria.clone(), true),
                    Tool::UnmarkMask => Command::MarkMask(side, criteria.clone(), false),
                    Tool::FilterOptions => {
                        criteria
                            .matching(std::iter::empty())
                            .map_err(|error| error.to_string())?;
                        self.interface.panes[side.index()].set_filter_text(&criteria.text);
                        Command::Filter(side, criteria.clone())
                    }
                    _ => unreachable!(),
                };
                Action::Command(command)
            }
            Kind::Bookmark => {
                if primary.trim().is_empty() {
                    return Err("Enter a bookmark name".into());
                }
                let path = path_input(secondary)?;
                let other = flags[0]
                    .then(|| path_input(&dialog.description))
                    .transpose()?;
                let bookmark = Bookmark {
                    name: primary.trim().to_owned(),
                    path,
                    opposite: other,
                    group: group.trim().to_owned(),
                };
                Action::Command(if let Tool::EditBookmark(index) = tool {
                    Command::EditBookmark(*index, bookmark)
                } else {
                    Command::AddBookmark(bookmark)
                })
            }
            Kind::Eject => {
                self.desktop.eject(&item()?)?;
                self.places_cache.expire(&());
                self.engine.dispatch(Command::Refresh);
                self.notice("Volume unmounted");
                return Ok(());
            }
            Kind::Inspect | Kind::HelpDetail | Kind::Favorites => return Ok(()),
        };
        self.apply(action, ctx);
        Ok(())
    }
}

enum Outcome {
    None,
    Submit(bool),
    Close,
    Browse,
    Favorite(usize, bool),
}

fn field(ui: &mut Ui, title: &str, value: &mut String, focus: &mut bool) {
    ui.label(label(title, 12., SUB).strong());
    let edit = ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
    if *focus {
        *focus = false;
        edit.request_focus();
        if let Some(mut state) = TextEdit::load_state(ui.ctx(), edit.id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(value.chars().count()),
                )));
            TextEdit::store_state(ui.ctx(), edit.id, state);
        }
    }
}

fn algorithm(ui: &mut Ui, algorithm: &mut Algorithm) {
    egui::ComboBox::from_label("Checksum algorithm")
        .selected_text(algorithm.label())
        .show_ui(ui, |ui| {
            for candidate in Algorithm::ALL {
                ui.selectable_value(algorithm, candidate, candidate.label());
            }
        });
}

fn draw_fields(ui: &mut Ui, dialog: &mut Dialog, outcome: &mut Outcome) {
    let Dialog {
        kind,
        tool,
        paths,
        description,
        primary,
        secondary,
        group,
        options,
        algorithm: checksum,
        manifest,
        numeric,
        flags,
        criteria,
        pack,
        extract,
        archive_target,
        focus,
        ..
    } = dialog;
    if !description.is_empty() && !matches!(kind, Kind::Bookmark | Kind::HelpDetail | Kind::Inspect)
    {
        ui.add(egui::Label::new(mono(description.as_str(), 12., SUB)).truncate())
            .on_hover_text(description.as_str());
    }
    match kind {
        Kind::Pack => {
            if let Some((archive, folder)) = archive_target {
                ui.label(archive.join(folder).display().to_string());
            } else {
                field(ui, "Archive path and extension", primary, focus);
                ui.checkbox(&mut pack.update, "Update existing archive");
            }
            ui.add(egui::Slider::new(&mut pack.level, 0..=9).text("Compression level"));
            ui.label("Password (optional)");
            ui.add(TextEdit::singleline(&mut pack.password).password(true));
            ui.label("Volume size (optional, for example 100m)");
            ui.add(TextEdit::singleline(&mut pack.volume));
        }
        Kind::Unpack => {
            field(ui, "Destination folder", primary, focus);
            if ui.small_button("Browse…").clicked() {
                *outcome = Outcome::Browse;
            }
            ui.label("File mask (optional)");
            ui.add(TextEdit::singleline(&mut extract.mask).hint_text("*.txt"));
            ui.checkbox(&mut extract.flatten, "Flatten paths");
            egui::ComboBox::from_id_salt("archive-layout")
                .selected_text(match extract.layout {
                    ExtractLayout::Direct => "Extract into destination",
                    ExtractLayout::Separate => "One folder per archive",
                    ExtractLayout::Smart => "Smart Extract",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut extract.layout,
                        ExtractLayout::Direct,
                        "Extract into destination",
                    );
                    ui.selectable_value(
                        &mut extract.layout,
                        ExtractLayout::Separate,
                        "One folder per archive",
                    );
                    ui.selectable_value(&mut extract.layout, ExtractLayout::Smart, "Smart Extract");
                });
            egui::ComboBox::from_id_salt("archive-overwrite")
                .selected_text(match extract.collision {
                    Collision::Replace => "Replace existing files",
                    Collision::Skip => "Skip existing files",
                    Collision::RenameIncoming => "Rename extracted files",
                    _ => "Ask before replacing",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut extract.collision,
                        Collision::Ask,
                        "Ask before replacing",
                    );
                    ui.selectable_value(
                        &mut extract.collision,
                        Collision::Replace,
                        "Replace existing files",
                    );
                    ui.selectable_value(
                        &mut extract.collision,
                        Collision::Skip,
                        "Skip existing files",
                    );
                    ui.selectable_value(
                        &mut extract.collision,
                        Collision::RenameIncoming,
                        "Rename extracted files",
                    );
                });
            ui.label("Password (optional)");
            ui.add(TextEdit::singleline(&mut extract.password).password(true));
        }
        Kind::DeleteArchiveMembers => {
            ui.label("Selected folders and their members will be removed from the archive.");
        }
        Kind::TestArchive => {
            ui.label("Password (optional)");
            ui.add(TextEdit::singleline(&mut extract.password).password(true));
        }
        Kind::UnlockArchive => {
            ui.label("Archive password");
            let response = ui.add(TextEdit::singleline(&mut extract.password).password(true));
            if std::mem::take(focus) {
                response.request_focus();
            }
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *outcome = Outcome::Submit(false);
            }
        }
        Kind::Text => {
            let hint = match tool {
                Tool::Comment => "Comment",
                Tool::Symlink | Tool::Hardlink | Tool::Shortcut => "New link path",
                Tool::SaveFavorite => "Workspace name",
                Tool::RenameTab(_) => "Tab title",
                _ => "Name",
            };
            field(ui, hint, primary, focus);
            if matches!(tool, Tool::Symlink) {
                ui.checkbox(&mut flags[0], "Use a relative target");
            }
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *outcome = Outcome::Submit(false);
            }
        }
        Kind::Transfer => {
            field(ui, "Destination folder", primary, focus);
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *outcome = Outcome::Submit(false);
            }
            if ui.small_button("Browse…").clicked() {
                *outcome = Outcome::Browse;
            }
            ui.collapsing("Transfer options", |ui| {
                ui.checkbox(&mut options.verify, "Verify every copied file");
                ui.checkbox(&mut options.check_space, "Check free space");
                ui.checkbox(&mut options.preallocate, "Preallocate space for each file");
                ui.checkbox(
                    &mut options.preserve_metadata,
                    "Preserve timestamps and permissions",
                );
                ui.checkbox(&mut options.follow_links, "Follow symbolic links");
                ui.checkbox(&mut options.adjust_links, "Adjust relative symbolic links");
                ui.checkbox(
                    &mut options.reflink,
                    "Try a filesystem clone before copying",
                );
                ui.checkbox(&mut options.omit_empty, "Omit newly created empty folders");
                let mut label_focus = false;
                field(
                    ui,
                    "Include file masks separated by ;",
                    &mut options.mask,
                    &mut label_focus,
                );
                field(
                    ui,
                    "Rename mask for top-level items",
                    &mut options.rename_mask,
                    &mut label_focus,
                );
                egui::ComboBox::from_label("Existing file")
                    .selected_text(collision_name(options.collision))
                    .show_ui(ui, |ui| {
                        for choice in [
                            Collision::Ask,
                            Collision::Skip,
                            Collision::Replace,
                            Collision::Newer,
                            Collision::Larger,
                            Collision::Smaller,
                            Collision::RenameIncoming,
                            Collision::RenameExisting,
                            Collision::Append,
                            Collision::Resume,
                        ] {
                            ui.selectable_value(
                                &mut options.collision,
                                choice,
                                collision_name(choice),
                            );
                        }
                    });
                egui::ComboBox::from_label("Existing folder")
                    .selected_text(collision_name(options.directory_collision))
                    .show_ui(ui, |ui| {
                        for choice in [
                            Collision::Ask,
                            Collision::Skip,
                            Collision::Merge,
                            Collision::RenameIncoming,
                            Collision::RenameExisting,
                        ] {
                            ui.selectable_value(
                                &mut options.directory_collision,
                                choice,
                                collision_name(choice),
                            );
                        }
                    });
            });
        }
        Kind::Delete => {
            let text = if matches!(tool, Tool::Trash) {
                "Items will be moved to the system Trash."
            } else {
                "The selected items and their contents will be permanently removed."
            };
            ui.label(label(
                text,
                14.,
                if matches!(tool, Tool::Trash) {
                    SUB
                } else {
                    RED
                },
            ));
            ui.checkbox(&mut flags[0], "Don't ask again for this action");
        }
        Kind::Wipe => {
            ui.label(label("File contents will be overwritten before deletion. Recovery may still be possible on SSDs, snapshots, and copy-on-write storage.",13.,RED));
            field(ui, "Overwrite passes", numeric, focus);
        }
        Kind::Inspect | Kind::HelpDetail => {
            egui::ScrollArea::vertical()
                .max_height(325.)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(mono(description.as_str(), 12., TEXT)).selectable(true),
                    );
                });
        }
        Kind::Attributes => {
            ui.checkbox(&mut flags[0], "Change read-only attribute");
            ui.add_enabled_ui(flags[0], |ui| {
                ui.checkbox(&mut flags[1], "Make read-only");
            });
            ui.checkbox(&mut flags[2], "Set modification time");
            ui.add_enabled_ui(flags[2], |ui| {
                let mut no_focus = false;
                field(ui, "Modification time", primary, &mut no_focus);
            });
            ui.checkbox(&mut flags[3], "Apply recursively to folders");
        }
        Kind::Split => {
            field(ui, "Destination folder", primary, focus);
            if ui.small_button("Browse…").clicked() {
                *outcome = Outcome::Browse;
            }
            ui.checkbox(&mut flags[0], "Choose number of parts");
            let mut no_focus = false;
            field(
                ui,
                if flags[0] {
                    "Number of parts"
                } else {
                    "Part size (MiB)"
                },
                numeric,
                &mut no_focus,
            );
        }
        Kind::Combine => {
            field(ui, "Output file", primary, focus);
            ui.label(label("Part order", 12., SUB).strong());
            let mut movement = None;
            let count = paths.len();
            egui::ScrollArea::vertical()
                .max_height(150.)
                .show(ui, |ui| {
                    for (index, path) in paths.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.push_id(index, |ui| {
                                if icon_button(ui, Icon::Up, "Move part earlier", index > 0)
                                    .clicked()
                                {
                                    movement = Some((index, index - 1));
                                }
                                if icon_button(
                                    ui,
                                    Icon::Download,
                                    "Move part later",
                                    index + 1 < count,
                                )
                                .clicked()
                                {
                                    movement = Some((index, index + 1));
                                }
                            });
                            ui.label(label(engine::display_name(path), 13., TEXT))
                                .on_hover_text(path.display().to_string());
                        });
                    }
                });
            if let Some((a, b)) = movement {
                paths.swap(a, b);
            }
        }
        Kind::Hash => {
            let previous = *checksum;
            algorithm(ui, checksum);
            if previous != *checksum && primary.ends_with(previous.extension()) {
                let next = Path::new(primary).with_extension(checksum.extension());
                *primary = next.display().to_string();
            }
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(manifest, ManifestMode::Single, "Single manifest");
                ui.selectable_value(manifest, ManifestMode::PerFile, "Per file");
                ui.selectable_value(manifest, ManifestMode::PerDirectory, "Per folder");
            });
            if *manifest == ManifestMode::Single {
                field(ui, "Manifest path", primary, focus);
            }
        }
        Kind::Verify => {
            algorithm(ui, checksum);
            ui.checkbox(
                &mut flags[0],
                "Read expected values from the selected manifest",
            );
            if !flags[0] {
                field(ui, "Expected checksum", primary, focus);
            }
        }
        Kind::Criteria => {
            field(
                ui,
                "Name, fragment, or wildcard mask",
                &mut criteria.text,
                focus,
            );
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut criteria.case_sensitive, "Case sensitive");
                ui.checkbox(&mut criteria.ignore_accents, "Ignore accents");
                ui.checkbox(&mut criteria.extension_only, "Extension only");
            });
            ui.horizontal_wrapped(|ui| {
                if matches!(tool, Tool::FilterOptions) {
                    ui.add_enabled(
                        flags[0],
                        egui::Checkbox::new(&mut criteria.recursive, "Recursive"),
                    )
                    .on_disabled_hover_text(
                        "Recursive filtering is available in folder and archive tabs",
                    );
                }
                ui.checkbox(&mut criteria.directories, "Directories");
                ui.checkbox(&mut criteria.files, "Files");
                ui.checkbox(&mut criteria.read_only, "Read-only items");
            });
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *outcome = Outcome::Submit(false);
            }
        }
        Kind::Bookmark => {
            field(ui, "Bookmark name", primary, focus);
            field(ui, "Folder", secondary, focus);
            field(ui, "Group (optional)", group, focus);
            ui.checkbox(&mut flags[0], "Open a paired folder in the opposite panel");
            if flags[0] {
                field(ui, "Opposite panel folder", description, focus);
            }
        }
        Kind::Eject => {
            ui.label(label("Unmount the selected volume?", 14., SUB));
        }
        Kind::Favorites => {}
    }
}

fn draw_favorites(
    ui: &mut Ui,
    dialog: &mut Dialog,
    favorites: &[engine::Favorite],
    outcome: &mut Outcome,
) {
    ui.checkbox(&mut dialog.flags[0], "Keep currently open tabs");
    if favorites.is_empty() {
        ui.label(label(
            "Save the open tabs from Tools → Workspace → Save favorite workspace.",
            13.,
            MUTED,
        ));
    }
    egui::ScrollArea::vertical()
        .max_height(280.)
        .show(ui, |ui| {
            for (index, favorite) in favorites.iter().enumerate() {
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        if action(
                            ui,
                            Icon::Star,
                            &favorite.name,
                            "Restore this workspace",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            *outcome = Outcome::Favorite(index, false);
                        }
                        if icon_button(ui, Icon::Trash, "Remove favorite", true).clicked() {
                            *outcome = Outcome::Favorite(index, true);
                        }
                    });
                });
            }
        });
}

fn collision_name(choice: Collision) -> &'static str {
    match choice {
        Collision::Ask => "Ask",
        Collision::Skip => "Skip",
        Collision::Replace => "Replace",
        Collision::Newer => "Replace if newer",
        Collision::Larger => "Replace if larger",
        Collision::Smaller => "Replace if smaller",
        Collision::RenameIncoming => "Rename incoming",
        Collision::RenameExisting => "Rename existing",
        Collision::Merge => "Merge folders",
        Collision::Append => "Append",
        Collision::Resume => "Resume partial file",
        Collision::Cancel => "Cancel",
    }
}
fn summarize(paths: &[PathBuf]) -> String {
    let names = paths
        .iter()
        .take(3)
        .map(|path| engine::display_name(path))
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > 3 {
        format!("{names} + {} more", paths.len() - 3)
    } else {
        names
    }
}
fn absolute_input(base: &Path, home: &Path, input: &str) -> Result<PathBuf, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a path".into());
    }
    let path = Path::new(input);
    Ok(if path == Path::new("~") {
        home.to_owned()
    } else if let Ok(relative) = path.strip_prefix("~/") {
        home.join(relative)
    } else {
        base.join(path)
    })
}
fn default_text_filename(base: &Path) -> String {
    let mut name = String::from("untitled.txt");
    let mut suffix = 1;
    while base.join(&name).symlink_metadata().is_ok() {
        name = format!("untitled{suffix}.txt");
        suffix += 1;
    }
    name
}

fn filename(input: &str) -> Result<&Path, String> {
    let path = Path::new(input);
    if input.trim().is_empty()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("Enter a single file or folder name".into());
    }
    Ok(path)
}
fn combine_dialog(side: Side, other: &Path, parts: Vec<PathBuf>) -> Dialog {
    let filename = parts[0].file_stem().unwrap_or_default().to_string_lossy();
    let output = other
        .join(format!("{filename}.joined"))
        .display()
        .to_string();
    let mut dialog = Dialog::new(Kind::Combine, Tool::Combine, side, "Combine files", parts);
    dialog.primary = output;
    dialog
}

fn describe(entry: &Entry) -> String {
    use std::fmt::Write;
    let fmt = |time: Option<std::time::SystemTime>| {
        time.map(|time| {
            chrono::DateTime::<chrono::Local>::from(time)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
    };
    let mut output = format!(
        "{}\n{}\n\nType  {}\nPermissions  {}\nSize  {} bytes",
        entry.name,
        entry.path.display(),
        entry.kind.label(),
        entry.attributes,
        entry.size
    );
    let _ = write!(
        output,
        "\nModified  {}\nCreated  {}\nAccessed  {}",
        fmt(entry.modified),
        fmt(entry.created),
        fmt(entry.accessed)
    );
    if !entry.comment.is_empty() {
        let _ = write!(output, "\nComment  {}", entry.comment);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn draw_dialog(app: &mut App, ctx: &egui::Context, key: Option<egui::Key>) {
        let events = key
            .into_iter()
            .flat_map(|key| {
                [true, false].map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
            })
            .collect();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100., 700.),
                )),
                events,
                ..Default::default()
            },
            |_| app.dialogs(ctx),
        );
        output.textures_delta.clear();
    }

    #[test]
    fn delete_confirmation_has_keyboard_focus_and_can_be_disabled() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, "").unwrap();
        std::fs::write(&second, "").unwrap();
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &creation,
            Box::new(crate::tests::TestDesktop::default()),
            root.clone(),
            [root.clone(), root],
            true,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.engine.tab(Side::Left).busy() {
            app.engine.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.engine
            .dispatch(Command::Select(Side::Left, first, engine::Select::Focus));
        app.invoke(Side::Left, Tool::Delete, &ctx);
        app.dialog.as_mut().unwrap().flags[0] = true;
        draw_dialog(&mut app, &ctx, None);
        draw_dialog(&mut app, &ctx, Some(egui::Key::Enter));
        assert!(app.dialog.is_none());
        assert!(app.engine.jobs.is_empty());
        assert!(app.engine.settings.confirm_delete);

        app.invoke(Side::Left, Tool::Delete, &ctx);
        app.dialog.as_mut().unwrap().flags[0] = true;
        draw_dialog(&mut app, &ctx, None);
        draw_dialog(&mut app, &ctx, Some(egui::Key::ArrowRight));
        draw_dialog(&mut app, &ctx, Some(egui::Key::Enter));
        assert!(app.dialog.is_none());
        assert_eq!(app.engine.jobs.len(), 1);
        assert!(!app.engine.settings.confirm_delete);
        assert!(app.engine.settings.confirm_trash);

        app.engine
            .dispatch(Command::Select(Side::Left, second, engine::Select::Focus));
        app.invoke(Side::Left, Tool::Delete, &ctx);
        assert!(app.dialog.is_none());
        assert_eq!(app.engine.jobs.len(), 2);
    }

    #[test]
    fn pack_and_copy_archive_members_use_archive_dialogs() {
        let archiver_found = ["7zz", "7z", "7za"].iter().any(|program| {
            std::process::Command::new(program)
                .arg("i")
                .output()
                .is_ok_and(|output| output.status.success())
        });
        if !archiver_found {
            return;
        }
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let file = root.join("note.txt");
        std::fs::write(&file, "Archive member").unwrap();
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &creation,
            Box::new(crate::tests::TestDesktop::default()),
            root.clone(),
            [root.clone(), destination.clone()],
            true,
        );
        let wait = |app: &mut App, job: bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                app.engine.poll();
                if !app.engine.tab(Side::Left).busy()
                    && (!job
                        || app
                            .engine
                            .jobs
                            .last()
                            .is_some_and(|job| job.state.finished()))
                {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "archive action timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait(&mut app, false);
        app.engine
            .dispatch(Command::Select(Side::Left, file, engine::Select::Focus));
        app.invoke(Side::Left, Tool::Pack, &ctx);
        let mut dialog = app.dialog.take().unwrap();
        assert!(dialog.kind == Kind::Pack);
        let password = "preview fixture password";
        dialog.pack.password = password.into();
        app.submit(&dialog, false, &ctx).unwrap();
        wait(&mut app, true);
        assert_eq!(
            app.engine.jobs.last().unwrap().state,
            engine::JobState::Done
        );

        app.apply(Action::Path(Side::Left, destination.join("note.zip")), &ctx);
        wait(&mut app, false);
        let entry = app.engine.tab(Side::Left).current().unwrap().clone();
        let locked_revision = entry.archive.as_ref().unwrap().revision;
        app.apply(Action::Open(Side::Left, entry, false), &ctx);
        assert_eq!(app.interface.quick, Some(Side::Left));
        app.invoke(Side::Left, Tool::Copy, &ctx);
        draw_dialog(&mut app, &ctx, None);
        draw_dialog(&mut app, &ctx, Some(egui::Key::Enter));
        assert!(app.dialog.is_none());
        wait(&mut app, true);
        let failed = app.engine.jobs.last().unwrap();
        assert_eq!(failed.state, engine::JobState::Failed);
        assert!(failed.message.starts_with("Password required"));
        assert!(!destination.join("note.txt").exists());
        app.invoke(Side::Left, Tool::UnlockArchive, &ctx);
        let mut dialog = app.dialog.take().unwrap();
        assert!(dialog.kind == Kind::UnlockArchive);
        dialog.extract.password = password.into();
        app.submit(&dialog, false, &ctx).unwrap();
        wait(&mut app, false);
        let source = app
            .engine
            .tab(Side::Left)
            .current()
            .unwrap()
            .archive
            .as_ref()
            .unwrap();
        assert_ne!(source.revision, locked_revision);
        assert!(matches!(
            source.preview(Path::new("note.txt"), engine::Kind::Text, 64),
            engine::Preview::Text(text) if text == "Archive member"
        ));
        app.invoke(Side::Left, Tool::Copy, &ctx);
        let dialog = app.dialog.as_ref().unwrap();
        assert!(dialog.kind == Kind::Unpack);
        assert_eq!(dialog.extract.password, password);
        draw_dialog(&mut app, &ctx, None);
        draw_dialog(&mut app, &ctx, Some(egui::Key::Enter));
        assert!(app.dialog.is_none());
        assert_eq!(app.engine.jobs.len(), 3);
        wait(&mut app, true);
        assert_eq!(
            app.engine.jobs.last().unwrap().state,
            engine::JobState::Done
        );
        assert_eq!(
            std::fs::read(destination.join("note.txt")).unwrap(),
            b"Archive member"
        );
    }

    #[test]
    fn details_export_uses_visible_order_and_marked_subset() {
        let root = tempfile::tempdir().unwrap();
        for name in ["file10.txt", "file2.txt", ".hidden"] {
            std::fs::write(root.path().join(name), "").unwrap();
        }
        std::fs::write(root.path().join("descript.ion"), "file2.txt one\ttwo\n").unwrap();
        let (send, wake) = std::sync::mpsc::channel();
        let mut engine = engine::Engine::new(
            root.path().into(),
            [root.path().into(), root.path().into()],
            engine::Settings {
                auto_refresh: false,
                ..Default::default()
            },
            move || {
                let _ = send.send(());
            },
        );
        while engine.tab(Side::Left).busy() {
            wake.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            engine.poll();
        }
        let settle = |engine: &mut engine::Engine| {
            while engine.tab(Side::Left).busy() {
                wake.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                engine.poll();
            }
        };
        let side = Side::Left;
        engine.dispatch(Command::Filter(side, MatchOptions::filename("file")));
        settle(&mut engine);
        let columns = &[Column::Name, Column::Comment];
        assert_eq!(
            details_text(engine.tab(side), columns, true, true),
            "file2.txt\tone two\nfile10.txt\t"
        );
        assert_eq!(
            details_text(engine.tab(side), columns, true, false),
            "file2.txt\tone two"
        );
        let tab = engine.tab(side);
        let modified =
            chrono::DateTime::<chrono::Local>::from(tab.current().unwrap().modified.unwrap());
        let modified_column = &[Column::Modified];
        assert_eq!(
            details_text(tab, modified_column, true, false),
            modified.format("%d %b %Y %H:%M").to_string()
        );
        assert_eq!(
            details_text(tab, modified_column, false, false),
            modified.format("%d %b %Y").to_string()
        );
        engine.dispatch(Command::Select(
            side,
            root.path().join("file10.txt"),
            engine::Select::Replace,
        ));
        assert_eq!(
            details_text(engine.tab(side), columns, true, true),
            "file10.txt\t"
        );
        engine.dispatch(Command::Select(
            side,
            root.path().join("file2.txt"),
            engine::Select::Toggle,
        ));
        assert_eq!(
            details_text(engine.tab(side), columns, true, true),
            "file2.txt\tone two\nfile10.txt\t"
        );
        engine.dispatch(Command::Sort(side, Column::Name, false));
        settle(&mut engine);
        assert_eq!(
            details_text(engine.tab(side), columns, true, true),
            "file10.txt\t\nfile2.txt\tone two"
        );
    }
}

#[cfg(test)]
mod workflow_tests;
