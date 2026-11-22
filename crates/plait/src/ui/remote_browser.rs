//! Opening a project on another machine by browsing its folders, through its
//! host, in the picker. A repository opens; any other folder opens to show
//! what is inside. A typed absolute path goes straight to that folder.
use super::{
    Plait,
    commands::Command,
    palette::{Item, Options, Palette, Typed},
    theme::colors,
};
use gpui::*;
use plait_core::host::folders::{self, Listing};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The picker's rows for one folder: open it when it is a repository, go up,
/// then its repositories and its other folders.
pub(super) fn browse_items(listing: &Listing) -> Vec<Item> {
    let mut items = Vec::new();
    let here = listing.path.display().to_string();
    if listing.git {
        items.push(
            Item::new("open-here", format!("Open {}", folder_name(&listing.path)), open(&listing.path))
                .detail(here.clone())
                .section("This folder")
                .glyph("folder", colors().mint),
        );
    }
    if let Some(parent) = &listing.parent {
        items.push(
            Item::new("up", format!("Up to {}", folder_name(parent)), browse(parent))
                .detail(parent.display().to_string())
                .section("This folder")
                .glyph("up", colors().muted),
        );
    }
    if listing.path != listing.home {
        items.push(
            Item::new("home", "Home folder", browse(&listing.home))
                .detail(listing.home.display().to_string())
                .section("This folder")
                .glyph("folder", colors().muted),
        );
    }
    for entry in &listing.entries {
        let path = listing.path.join(&entry.name);
        let item = Item::new(format!("folder:{}", entry.name), entry.name.clone(), if entry.git {
            open(&path)
        } else {
            browse(&path)
        });
        items.push(if entry.git {
            item.detail("Repository")
                .section("Repositories")
                .glyph("folder", colors().mint)
        } else {
            item.section("Folders").glyph("folder", colors().subtle)
        });
    }
    if listing.truncated {
        items.push(
            Item::new("more", "More folders than shown; type a path to go further", browse(&listing.path))
                .section("Folders")
                .enabled(false),
        );
    }
    if items.is_empty() {
        items.push(
            Item::new("empty", "No folders here", browse(&listing.path))
                .detail(here)
                .enabled(false),
        );
    }
    items
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn open(path: &Path) -> Command {
    Command::OpenRemoteProject(path.display().to_string())
}

fn browse(path: &Path) -> Command {
    Command::BrowseRemote(path.to_owned())
}

/// A single row that says what is happening or went wrong.
fn notice(key: &'static str, text: String) -> Vec<Item> {
    vec![Item::new(key, text, Command::BrowseRemote(PathBuf::new())).enabled(false)]
}

impl Plait {
    /// Opens the picker on the remote machine's home folder.
    pub(super) fn open_remote_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let machine = self.machine_label();
        let options = Options {
            name: format!("Open a project on {machine}").into(),
            placeholder: "Filter folders, or type a path…".into(),
            empty: "No folders here.".into(),
            initial: None,
            mark: None,
            aside: None,
            note: None,
        };
        // Until a host answers that it can list folders, a typed path opens directly.
        let browsable = Rc::new(Cell::new(true));
        let typed: Typed = {
            let browsable = browsable.clone();
            Rc::new(move |text, _| {
                let valid = super::valid_remote_path(text);
                let (title, command) = if browsable.get() {
                    (format!("Go to {text}"), Command::BrowseRemote(PathBuf::from(text)))
                } else {
                    (format!("Open {text}"), Command::OpenRemoteProject(text.into()))
                };
                let title = if valid {
                    title
                } else {
                    "Type an absolute path, such as /home/you/project.".into()
                };
                Some((Item::new("typed-path", title, command).enabled(valid), true))
            })
        };
        let palette = cx.new(|cx| {
            let mut palette = Palette::new(options, Vec::new(), window, cx);
            palette.set_typed(typed, cx);
            palette
        });
        self.remote_browser = Some(browsable);
        self.present(palette, None, window, cx);
        self.browse_remote(None, cx);
    }

    /// Lists `path` (the home folder when `None`) into the open picker.
    pub(super) fn browse_remote(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(palette) = self.overlay.as_ref().map(|overlay| overlay.palette.clone()) else {
            return;
        };
        let machine = self.machine_label();
        palette.update(cx, |palette, cx| {
            palette.clear_query(cx);
            palette.set_items(notice("reading", format!("Reading folders on {machine}…")), cx);
        });
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { folders::browse(&client, path.as_deref(), false) });
        let browsable = self.remote_browser.clone();
        cx.spawn(async move |_, cx| {
            let result = task.await;
            let items = match result {
                Ok(listing) => browse_items(&listing),
                Err(error) if folders::needs_host_update(&error) => {
                    if let Some(browsable) = browsable {
                        browsable.set(false);
                    }
                    notice(
                        "older-host",
                        format!("Update {machine}'s host to browse its folders. Type a path to open one."),
                    )
                }
                Err(error) => notice("failed", error.to_string()),
            };
            let _ = palette.update(cx, |palette, cx| palette.set_items(items, cx));
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_core::host::folders::Entry;

    fn listing(git: bool) -> Listing {
        Listing {
            path: "/home/me/code".into(),
            parent: Some("/home/me".into()),
            home: "/home/me".into(),
            git,
            entries: vec![
                Entry {
                    name: "api".into(),
                    git: true,
                },
                Entry {
                    name: "notes".into(),
                    git: false,
                },
            ],
            truncated: false,
        }
    }

    fn keys(items: &[Item]) -> Vec<String> {
        items.iter().map(|item| item.key.to_string()).collect()
    }

    #[core::prelude::v1::test]
    fn a_repository_opens_and_a_folder_opens_to_its_contents() {
        let items = browse_items(&listing(false));
        assert_eq!(keys(&items), ["up", "home", "folder:api", "folder:notes"]);
        assert!(matches!(&items[2].command, Command::OpenRemoteProject(p) if p == "/home/me/code/api"));
        assert!(matches!(&items[3].command, Command::BrowseRemote(p) if p == Path::new("/home/me/code/notes")));
        assert!(matches!(&items[0].command, Command::BrowseRemote(p) if p == Path::new("/home/me")));
    }

    #[core::prelude::v1::test]
    fn a_folder_that_is_a_repository_offers_to_open_itself_first() {
        let items = browse_items(&listing(true));
        assert_eq!(items[0].key.as_ref(), "open-here");
        assert!(matches!(&items[0].command, Command::OpenRemoteProject(p) if p == "/home/me/code"));
    }

    #[core::prelude::v1::test]
    fn the_home_folder_and_the_root_have_no_redundant_rows() {
        let mut root = listing(false);
        root.path = "/".into();
        root.parent = None;
        root.home = "/".into();
        root.entries.clear();
        assert_eq!(keys(&browse_items(&root)), ["empty"]);
        let mut many = listing(false);
        many.truncated = true;
        assert!(!browse_items(&many).last().unwrap().enabled);
    }
}
