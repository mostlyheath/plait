//! Opening a project on another machine by browsing its folders, through its
//! host, in the picker. Any folder can be opened; a repository opens in one
//! step from the list, and another folder opens to show what is inside. A
//! typed absolute path goes straight to that folder.
use super::{
    Plait,
    commands::Command,
    palette::{Item, Options, Palette, Typed},
    theme::colors,
};
use gpui::*;
use plait_client::host::folders::{self, Listing};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The picker's rows for one folder: open it, go up, then its repositories
/// and its other folders.
pub(super) fn browse_items(listing: &Listing) -> Vec<Item> {
    let mut items = Vec::new();
    let here = listing.path.display().to_string();
    // A folder that is not a repository opens too, without Git's features.
    let (detail, tint) = if listing.git {
        (here.clone(), colors().mint)
    } else {
        (format!("{here} · not a Git repository"), colors().subtle)
    };
    items.push(
        Item::new(
            "open-here",
            format!("Open {}", folder_name(&listing.path)),
            open(&listing.path),
        )
        .detail(detail)
        .section("This folder")
        .glyph("folder", tint),
    );
    if let Some(parent) = &listing.parent {
        items.push(
            Item::new(
                "up",
                format!("Up to {}", folder_name(parent)),
                browse(parent),
            )
            .detail(parent.display().to_string())
            .section("This folder")
            .glyph("up", colors().muted),
        );
    }
    if listing.path != listing.home && listing.parent.as_ref() != Some(&listing.home) {
        items.push(
            Item::new("home", "Home folder", browse(&listing.home))
                .detail(listing.home.display().to_string())
                .section("This folder")
                .glyph("folder", colors().muted),
        );
    }
    // Repositories first, then other folders, each in the host's order.
    let (repositories, folders): (Vec<_>, Vec<_>) =
        listing.entries.iter().partition(|entry| entry.git);
    for entry in repositories.into_iter().chain(folders) {
        let path = listing.path.join(&entry.name);
        let item = Item::new(
            format!("folder:{}", entry.name),
            entry.name.clone(),
            if entry.git {
                open(&path)
            } else {
                browse(&path)
            },
        );
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
            Item::new(
                "more",
                "More folders than shown; type a path to go further",
                browse(&listing.path),
            )
            .section("Folders")
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

fn reply_is_current(generation: u64, current: u64, epoch: u64, current_epoch: u64) -> bool {
    generation == current && epoch == current_epoch
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
                // Nothing typed is filtering, not a path.
                if text.trim().is_empty() {
                    return None;
                }
                let valid = super::valid_remote_path(text);
                let (title, command) = if browsable.get() {
                    (
                        format!("Go to {text}"),
                        Command::BrowseRemote(PathBuf::from(text)),
                    )
                } else {
                    (
                        format!("Open {text}"),
                        Command::OpenRemoteProject(text.into()),
                    )
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
        self.remote_listing = None;
        self.present(palette, None, window, cx);
        self.browse_remote(None, cx);
    }

    /// Lists `path` (the home folder when `None`) into the open picker.
    pub(super) fn browse_remote(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(palette) = self.overlay.as_ref().map(|overlay| overlay.palette.clone()) else {
            return;
        };
        self.remote_browser_generation = self.remote_browser_generation.wrapping_add(1);
        let generation = self.remote_browser_generation;
        let epoch = self.host.connection.epoch;
        let machine = self.machine_label();
        let initial = self.remote_listing.is_none();
        palette.update(cx, |palette, cx| {
            if initial {
                palette.set_items(
                    notice("reading", format!("Reading folders on {machine}…")),
                    cx,
                );
            }
            palette.set_note(Some(format!("Reading folders on {machine}…").into()), cx);
        });
        let client = self.host_client();
        let retry = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { folders::browse(&client, path.as_deref(), false) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if !reply_is_current(generation, this.remote_browser_generation, epoch, this.host.connection.epoch)
                    || !this.overlay.as_ref().is_some_and(|overlay| overlay.palette == palette) {
                    return;
                }
                match result {
                    Ok(listing) => {
                        let items = browse_items(&listing);
                        this.remote_listing = Some(listing);
                        palette.update(cx, |palette, cx| {
                            palette.clear_query(cx);
                            palette.set_note(None, cx);
                            palette.set_items(items, cx);
                        });
                    }
                    Err(error) if folders::needs_host_update(&error) => {
                        if let Some(browsable) = &this.remote_browser { browsable.set(false); }
                        palette.update(cx, |palette, cx| {
                            palette.set_items(notice("older-host", format!("Update {machine}'s host to browse its folders. Type a path to open one.")), cx);
                            palette.set_note(None, cx);
                        });
                    }
                    Err(error) => {
                        let mut items = this.remote_listing.as_ref().map(browse_items).unwrap_or_default();
                        let failed_query = retry.as_ref().map(|path| path.display().to_string()).unwrap_or_default();
                        let target = if failed_query.is_empty() { "Home folder" } else { &failed_query };
                        items.insert(0, Item::new("retry", "Retry", Command::BrowseRemote(retry.unwrap_or_default())).detail(target.to_owned()));
                        palette.update(cx, |palette, cx| {
                            palette.set_recovery_items(items, &failed_query, cx);
                            palette.set_note(Some(error.to_string().into()), cx);
                        });
                    }
                }
            });
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_client::host::folders::Entry;

    #[core::prelude::v1::test]
    fn stale_requests_and_old_connections_cannot_replace_a_listing() {
        assert!(reply_is_current(2, 2, 4, 4));
        assert!(!reply_is_current(1, 2, 4, 4));
        assert!(!reply_is_current(2, 2, 3, 4));
    }

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
        // The parent is the home folder, so there is no separate home row.
        assert_eq!(
            keys(&items),
            ["open-here", "up", "folder:api", "folder:notes"]
        );
        assert!(items[0].detail.contains("not a Git repository"));
        assert!(
            matches!(&items[2].command, Command::OpenRemoteProject(p) if p == "/home/me/code/api")
        );
        assert!(
            matches!(&items[3].command, Command::BrowseRemote(p) if p == Path::new("/home/me/code/notes"))
        );
        assert!(
            matches!(&items[1].command, Command::BrowseRemote(p) if p == Path::new("/home/me"))
        );
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
        assert_eq!(keys(&browse_items(&root)), ["open-here"]);
        let mut many = listing(false);
        many.truncated = true;
        assert!(!browse_items(&many).last().unwrap().enabled);
    }
}
