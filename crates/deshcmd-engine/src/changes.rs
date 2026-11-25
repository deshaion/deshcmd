//! Conservative job effects, derived on the job worker before any mutation.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{ArchiveOperation, Location, ManifestMode, Operation};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Scope {
    Item,
    Directory,
    Branch,
}

#[derive(Default)]
pub(crate) struct Effects {
    paths: HashSet<(PathBuf, Scope)>,
    uncertain: bool,
}

impl Effects {
    pub fn for_operation(operation: &Operation) -> Self {
        use Scope::{Branch, Directory, Item};

        let mut effects = Self::default();
        let mut resolved = HashMap::new();
        let mut add = |path: &Path, scope| {
            if effects.uncertain {
                return;
            }
            if effects.paths.len() >= 512 || !path.is_absolute() {
                effects.paths.clear();
                effects.uncertain = true;
                return;
            }
            effects.paths.insert((path.to_owned(), scope));
            let sidecar = path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("descript.ion"));
            // Jobs may be given paths through a directory symlink. Resolve parent
            // directories while the old names still exist, without following a deleted link.
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                let real = resolved
                    .entry(parent.to_owned())
                    .or_insert_with(|| parent.canonicalize().ok());
                if sidecar {
                    effects.paths.insert((parent.to_owned(), Directory));
                    if let Some(real) = real {
                        effects.paths.insert((real.clone(), Directory));
                    }
                }
                if let (Some(real), Some(name)) = (real, path.file_name()) {
                    effects.paths.insert((real.join(name), scope));
                }
            }
            if scope != Item
                && let Some(real) = path.canonicalize().ok()
            {
                effects.paths.insert((real, scope));
            }
        };
        match operation {
            Operation::Archive(ArchiveOperation::Pack {
                output, options, ..
            }) => {
                if !options.volume.is_empty()
                    && let Some(parent) = output.parent()
                {
                    add(parent, Directory);
                } else {
                    add(output, Item);
                }
            }
            Operation::Archive(ArchiveOperation::Delete(crate::ArchiveSelection {
                file: output,
                ..
            }))
            | Operation::Create(output)
            | Operation::Combine { output, .. }
            | Operation::Export { output, .. } => add(output, Item),
            Operation::Transfer {
                paths,
                destination,
                moving,
                ..
            } => {
                add(destination, Branch);
                if *moving {
                    for path in paths {
                        add(path, Branch);
                    }
                }
            }
            Operation::Archive(ArchiveOperation::Extract { destination, .. }) => {
                add(destination, Branch)
            }
            Operation::Rename { source, target } => {
                add(source, Branch);
                add(target, Branch);
            }
            Operation::Mkdir(path) => {
                add(path, Item);
                for parent in path
                    .ancestors()
                    .skip(1)
                    .take_while(|parent| !parent.is_dir())
                {
                    add(parent, Item);
                }
            }
            Operation::Delete { paths, .. } | Operation::Wipe { paths, .. } => {
                for path in paths {
                    add(path, Branch);
                }
            }
            Operation::Link {
                source,
                target,
                symbolic,
                ..
            } => {
                add(target, Item);
                if !symbolic {
                    add(source, Item);
                }
            }
            Operation::Properties {
                paths, recursive, ..
            } => {
                for path in paths {
                    let scope = if *recursive { Branch } else { Item };
                    add(path, scope);
                    if let Ok(target) = path.canonicalize() {
                        add(&target, scope);
                    }
                }
            }
            Operation::Comment { path, .. } => {
                if let Some(directory) = path.parent() {
                    add(directory, Directory);
                }
            }
            Operation::Split { destination, .. } => add(destination, Directory),
            Operation::Hash {
                paths,
                output,
                mode,
                ..
            } => {
                if *mode == ManifestMode::Single {
                    add(output, Item);
                } else {
                    for path in paths {
                        add(path, Branch);
                        if let Some(parent) = path.parent() {
                            add(parent, Directory);
                        }
                    }
                }
            }
            Operation::Archive(ArchiveOperation::Test { .. })
            | Operation::Statistics(_)
            | Operation::Verify { .. }
            | Operation::Benchmark => {}
        }
        effects
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.uncertain
    }

    pub fn directory(&self, directory: &Path) -> bool {
        self.uncertain
            || self.paths.iter().any(|(path, scope)| {
                path.parent() == Some(directory)
                    || (directory == path && *scope != Scope::Item)
                    || (*scope == Scope::Branch && directory.starts_with(path))
            })
    }

    pub fn item(&self, item: &Path) -> bool {
        self.uncertain
            || self.paths.iter().any(|(path, scope)| {
                item == path
                    || (item.parent() == Some(path) && *scope == Scope::Directory)
                    || (*scope == Scope::Branch && item.starts_with(path))
            })
    }

    fn tree(&self, root: &Path) -> bool {
        self.uncertain
            || self.paths.iter().any(|(path, scope)| {
                path.starts_with(root) || (*scope == Scope::Branch && root.starts_with(path))
            })
    }

    pub fn directory_listing(&self, path: &Path, recursive: bool) -> bool {
        if recursive {
            self.tree(path)
        } else {
            self.directory(path)
        }
    }

    pub fn listing(&self, location: &Location, recursive: bool) -> bool {
        match location {
            Location::Directory(path) => self.directory_listing(path, recursive),
            Location::Flat { roots, .. } => {
                roots.iter().any(|root| self.tree(root) || self.item(root))
            }
            Location::List { paths, .. } => paths.iter().any(|path| self.item(path)),
            Location::Archive { file, .. } => self.item(file),
        }
    }
}
