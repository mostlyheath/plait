//! The project's Linear issues, in a tab of their own. The host makes every
//! Linear request; this view asks for pages and details off the UI thread and
//! draws Linear's Markdown read-only (`issue_text`). No link inside an issue is
//! followed or fetched; Open in Linear opens the issue's own https address in
//! the browser. The one write here, a status change, runs only on a person's
//! choice and is recorded by the host before it is sent.
use super::{
    Plait,
    composer::{Composer, ComposerEvent},
    issue_text::{self, IssueText, Picture, READING_WIDTH},
    settings,
    tab_lifecycle::Intent,
    text_field::TextField,
    text_field::TextFieldEvent,
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::{
    host::linear::{self, IssueDetail, IssueSummary},
    linear_drafts::{self, Drafts, SavedForm},
    linear_issues::{
        self, FILTERS, FormDone, FormKind, FormSlot, FormStatus, IssueForm, IssueRef, IssuesView,
        ListRequest, Load, Submit,
    },
    linear_settings::{self, Auth},
    tabs::{TabTarget, bookmark::BookmarkRecord},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

actions!(plait, [Issues]);

/// Typing pauses this long before the list is asked for again.
const SEARCH_PAUSE: Duration = Duration::from_millis(350);

#[derive(Default)]
pub(super) struct IssuesUi {
    pub view: IssuesView,
    pub search: Option<Entity<TextField>>,
    _search_events: Option<Subscription>,
    search_generation: u64,
    /// The selection the restored tab asked for, applied on its next read.
    restored: Option<String>,
    /// Drafts started from an issue, by draft ID: linked once the host creates the thread.
    pub pending_links: HashMap<String, IssueRef>,
    /// The issue offered to "Add to thread…" while its picker is open.
    offer: Option<(IssueRef, String)>,
    /// Each thread's source issue, read once per connection.
    pub links: HashMap<String, linear::Link>,
    links_epoch: Option<u64>,
    /// The shown issue's Markdown, parsed off the UI thread.
    text: Option<Arc<IssueText>>,
    /// The revision being parsed, so one issue is parsed once.
    parsing: Option<(String, String)>,
    /// `linear_info` is in flight.
    checking_writes: bool,
    /// Images on Linear's storage for the shown issue, by address.
    pub images: HashMap<String, Picture>,
    /// The connection, sign-in and key the images were read under.
    images_owner: Option<(u64, u64, String, String)>,
    /// Bumped when the sign-in changes, so earlier reads are dropped.
    image_generation: u64,
    /// Image reads in flight.
    image_reads: usize,
    /// Decoded bytes held per image, and the images in the order last shown.
    image_bytes: HashMap<String, usize>,
    image_order: Vec<String>,
    /// The open form's fields, made when it opens and dropped when it closes.
    form_fields: HashMap<FormSlot, FormFields>,
    /// Every project's unsaved forms as last written, once read from disk.
    drafts: Option<Drafts>,
    drafts_loading: bool,
    /// Bumped on each change to the drafts; writes carry it.
    drafts_revision: u64,
    /// Writes one at a time, so an older snapshot never replaces a newer.
    drafts_writer: Arc<linear_drafts::Writer>,
    /// The project whose saved forms were last reopened.
    restored_for: Option<PathBuf>,
    /// The issue whose exact text is being read for an edit.
    reading_text: Option<String>,
    /// The last form was refused because the drafts are at their limit.
    drafts_full: bool,
    /// Why the last Edit did not open, by issue.
    edit_note: Option<(String, String)>,
}

/// The fields of a new issue or an edit. Enter in the description saves.
struct FormFields {
    title: Entity<TextField>,
    description: Entity<Composer>,
    _subscriptions: [Subscription; 3],
}

/// Typing pauses this long before unsaved issue text is written to disk.
const DRAFT_PAUSE: Duration = Duration::from_millis(500);

impl IssuesUi {
    /// The open form's title and description, for scenes to type into.
    #[cfg(test)]
    pub(super) fn form_fields_for_scene(&self) -> Option<(Entity<TextField>, Entity<Composer>)> {
        let fields = self.form_fields.values().next()?;
        Some((fields.title.clone(), fields.description.clone()))
    }
}

impl IssuesUi {
    pub(super) fn decoded_bytes(&self) -> usize {
        self.image_bytes.values().sum()
    }

    /// Lets go of the least recently shown pixels until `room` more bytes fit
    /// the bound, never the shown issue's.
    fn evict_images(&mut self, visible: &[String], room: usize) {
        while self.decoded_bytes() + room > IMAGE_MEMORY {
            let Some(at) = self
                .image_order
                .iter()
                .position(|url| !visible.contains(url))
            else {
                break;
            };
            let url = self.image_order.remove(at);
            self.image_bytes.remove(&url);
            self.images.remove(&url);
        }
    }

    /// Keeps a decoded image if it fits the bound, making room from other
    /// issues' images first. One that still does not fit is shown in Linear.
    pub(super) fn admit_image(
        &mut self,
        url: String,
        pixels: Arc<RenderImage>,
        bytes: usize,
        visible: &[String],
    ) {
        self.evict_images(visible, bytes);
        if self.decoded_bytes() + bytes > IMAGE_MEMORY {
            self.images.insert(url, Picture::Elsewhere);
            return;
        }
        self.image_bytes.insert(url.clone(), bytes);
        self.image_order.retain(|held| held != &url);
        self.image_order.push(url.clone());
        self.images.insert(url, Picture::Ready(pixels));
    }
}

/// Images read at once, so a long issue does not queue every one together.
const IMAGE_READS: usize = 3;
/// Longest side an issue image is decoded to; it is drawn no wider than the text.
const IMAGE_EDGE: u32 = 1600;
/// Decoded issue images held at once, across every issue.
const IMAGE_MEMORY: usize = 48 * 1024 * 1024;

/// Where a reference is waiting to be added.
#[derive(Clone, PartialEq, Eq)]
enum Destination {
    Draft(String),
    Thread(String),
}

impl Plait {
    pub(super) fn issues_visible(&self) -> bool {
        self.tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .is_some_and(|tab| tab.session().target().is_issues())
    }

    pub(super) fn open_issues(
        &mut self,
        project: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        if self.settings_open {
            self.show_settings(false, window, cx);
        }
        let project = self.project_of(&project).to_owned();
        let target = TabTarget::Issues {
            checkout: self.tab_checkout(project),
        };
        // A project the sidebar has verified opens at once, as a thread does.
        // Asking the host again would only delay the tab, and a newer tab
        // request meanwhile would drop it.
        if self.tabs.identity.verified && target.checkout().directory.is_some() {
            self.request_tab(Intent::Open(target), window, cx);
        } else {
            self.open_tab_target(target, window, cx);
        }
    }

    pub(super) fn restore_issues_bookmark(&mut self) {
        let bookmark = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .map(|tab| tab.session().bookmark());
        self.issues.restored = match bookmark {
            Some(BookmarkRecord::Issues { selected }) => selected.clone(),
            _ => None,
        };
    }

    fn save_issues_bookmark(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.model.active() {
            let result = self.tabs.model.save_bookmark(
                id,
                BookmarkRecord::Issues {
                    selected: self.issues.view.selected.clone(),
                },
            );
            self.tab_error(result);
            self.queue_tab_save(cx);
        }
    }

    fn issues_search_field(&mut self, cx: &mut Context<Self>) {
        if self.issues.search.is_some() {
            return;
        }
        let field = cx.new(|cx| {
            TextField::new("Search issues", "Search titles or paste an issue ID", cx).search()
        });
        self.issues._search_events =
            Some(cx.subscribe(&field, |this, _, _: &TextFieldEvent, cx| {
                this.schedule_issue_search(cx)
            }));
        self.issues.search = Some(field);
    }

    /// Runs the search once typing pauses, so each keystroke is not a request.
    fn schedule_issue_search(&mut self, cx: &mut Context<Self>) {
        self.issues.search_generation += 1;
        let generation = self.issues.search_generation;
        let timer = cx.background_executor().timer(SEARCH_PAUSE);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.issues.search_generation != generation {
                    return;
                }
                let text = this
                    .issues
                    .search
                    .as_ref()
                    .map(|field| field.read(cx).text().to_owned())
                    .unwrap_or_default();
                if this.issues.view.set_search(&text) {
                    this.load_issue_list(false, cx);
                    this.resolve_issue_reference(&text, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Keeps the view pointed at the visible tab's project and asks the host for
    /// whatever it is missing. Called on every notify; each step runs once.
    pub(super) fn sync_issues_view(&mut self, cx: &mut Context<Self>) {
        if !self.issues_visible() {
            return;
        }
        self.issues_search_field(cx);
        let checkout = self.visible_project_checkout().to_owned();
        let restored = self.issues.restored.take();
        if self
            .issues
            .view
            .target(self.host.connection.epoch, &checkout, restored)
        {
            self.issues.restored_for = None;
        }
        self.parse_issue_text(cx);
        self.load_issue_images(cx);
        self.load_saved_forms(cx);
        self.restore_saved_forms(cx);
        // Fields live as long as their form; a closed or reset form lets them go.
        let view = &self.issues.view;
        let open = |slot: &FormSlot| match slot {
            FormSlot::New => view.new_form.is_some(),
            FormSlot::Edit(id) => view.edits.contains_key(id),
        };
        self.issues.form_fields.retain(|slot, _| open(slot));
        if cfg!(test)
            || !self.host.connection.connected
            || !matches!(self.linear.account.auth, Auth::SignedIn { .. })
        {
            return;
        }
        self.load_write_support(cx);
        if self.issues.view.needs_binding() {
            let generation = self.issues.view.start_binding();
            self.linear_task(
                move |client| {
                    linear::get_project_binding(client, checkout)
                        .map_err(|error| linear::classify(&error, false))
                },
                move |this, result, _| this.issues.view.binding_reply(generation, result),
                cx,
            );
        }
        if self.issues.view.needs_list() {
            self.load_issue_list(false, cx);
        }
        if self.issues.view.needs_detail()
            && let Some((id, generation)) = self.issues.view.reload_detail()
        {
            self.load_issue_detail(id, generation, cx);
        }
    }

    /// Parses the shown issue's Markdown once per revision, off the UI thread.
    fn parse_issue_text(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = self.issues.view.detail.as_ref().map(|d| d.issue.clone()) else {
            return;
        };
        let key = (detail.summary.id.clone(), detail.summary.updated_at.clone());
        if self
            .issues
            .text
            .as_ref()
            .is_some_and(|t| t.matches(&detail))
            || self.issues.parsing.as_ref() == Some(&key)
        {
            return;
        }
        // Scenes draw in one pass; the parse is small and has no I/O.
        if cfg!(test) {
            self.issues.text = Some(Arc::new(IssueText::parse(&detail)));
            return;
        }
        self.issues.parsing = Some(key.clone());
        let task = cx
            .background_executor()
            .spawn(async move { IssueText::parse(&detail) });
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.issues.parsing.as_ref() != Some(&key) {
                    return;
                }
                this.issues.parsing = None;
                this.issues.text = Some(Arc::new(text));
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether this host records Linear writes durably, and the linked team's
    /// workflow states once it does. An older host offers no status control.
    fn load_write_support(&mut self, cx: &mut Context<Self>) {
        if self.issues.view.writes.is_none() && !self.issues.checking_writes {
            self.issues.checking_writes = true;
            self.linear_task(
                |client| linear::info(client).map_err(|error| linear::classify(&error, true)),
                |this, result, _| {
                    this.issues.checking_writes = false;
                    let features = result.ok().map(|info| info.features);
                    let view = &mut this.issues.view;
                    view.writes = Some(features.as_ref().is_some_and(|f| f.durable_write_outcomes));
                    view.issue_writes = features.as_ref().is_some_and(|f| f.issue_writes);
                    view.images = features.as_ref().is_some_and(|f| f.images);
                    true
                },
                cx,
            );
        }
        if self.issues.view.needs_states() {
            let generation = self.issues.view.start_states();
            self.linear_task(
                |client| {
                    linear::list_teams(client, false)
                        .map_err(|error| linear::classify(&error, false))
                },
                move |this, result, _| this.issues.view.states_reply(generation, result),
                cx,
            );
        }
    }

    fn run_issue_list(&mut self, request: ListRequest, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let call = request.clone();
        self.linear_task(
            move |client| {
                linear::list_issues(
                    client,
                    call.team_id,
                    call.filter,
                    call.search,
                    call.cursor,
                    call.refresh,
                )
                .map_err(|error| linear::classify(&error, false))
            },
            move |this, result, _| this.issues.view.list_reply(&request, result),
            cx,
        );
    }

    fn load_issue_list(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if let Some(request) = self.issues.view.start_list(refresh) {
            self.run_issue_list(request, cx);
        }
    }

    fn load_more_issues(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.issues.view.start_more() {
            self.run_issue_list(request, cx);
            cx.notify();
        }
    }

    fn load_issue_detail(&mut self, id: String, generation: u64, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let workspace = self.issues.view.team().and_then(|b| b.workspace_id.clone());
        let requested = id.clone();
        self.linear_task(
            move |client| {
                linear::get_issue(client, requested, None, false, workspace)
                    .map_err(|error| linear::classify(&error, false))
            },
            move |this, result, _| this.issues.view.detail_reply(generation, &id, result),
            cx,
        );
    }

    pub(super) fn select_issue(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(generation) = self.issues.view.select(id) {
            self.save_issues_bookmark(cx);
            self.load_issue_detail(id.to_owned(), generation, cx);
        }
        cx.notify();
    }

    /// Reads the shown issue's Linear images through the host, a few at a
    /// time, and decodes them off the UI thread. Decoded images of every issue
    /// share one memory bound: the least recently shown go first, and the
    /// shown issue's are kept over others. An image let go is read again from
    /// the host's disk cache when it is shown. Images read under another
    /// sign-in are let go and their pending reads ignored.
    fn load_issue_images(&mut self, cx: &mut Context<Self>) {
        let owner = match &self.linear.account.auth {
            Auth::SignedIn { workspace, viewer } => Some((
                self.host.connection.epoch,
                self.linear.account.credentials,
                workspace.to_string(),
                viewer.to_string(),
            )),
            _ => None,
        };
        if self.issues.images_owner != owner {
            self.issues.images_owner = owner;
            self.issues.images.clear();
            self.issues.image_bytes.clear();
            self.issues.image_generation += 1;
            self.issues.image_reads = 0;
        }
        let Some(text) = self.issues.text.clone() else {
            return;
        };
        let visible = text.linear_images();
        // Reads that never finished for another issue are let go; pixels stay.
        self.issues
            .images
            .retain(|url, picture| matches!(picture, Picture::Ready(_)) || visible.contains(url));
        // Shown now: the most recently used.
        self.issues.image_order.retain(|url| !visible.contains(url));
        self.issues.image_order.extend(
            visible
                .iter()
                .filter(|url| self.issues.image_bytes.contains_key(*url))
                .cloned(),
        );
        self.evict_images(&visible);
        if cfg!(test)
            || !self.issues.view.images
            || !self.host.connection.connected
            || self.issues.images_owner.is_none()
        {
            return;
        }
        let wanted: Vec<_> = visible
            .into_iter()
            .filter(|url| !self.issues.images.contains_key(url))
            .take(IMAGE_READS.saturating_sub(self.issues.image_reads))
            .collect();
        for url in wanted {
            // The shown issue alone fills the bound: later images stay in Linear.
            if self.decoded_bytes() >= IMAGE_MEMORY {
                break;
            }
            self.issues.images.insert(url.clone(), Picture::Loading);
            self.issues.image_reads += 1;
            let generation = self.issues.image_generation;
            let read = url.clone();
            self.linear_task(
                move |client| {
                    let bytes =
                        linear::read_image(client, &read).map_err(
                            |error| match linear::classify(&error, false) {
                                linear::Rejection::NotFound => {
                                    "Not readable with this key".to_owned()
                                }
                                linear::Rejection::Other => {
                                    "Not a PNG or JPEG Plait can show".to_owned()
                                }
                                _ => "Could not be read".to_owned(),
                            },
                        )?;
                    super::images::decode_counted(&bytes, IMAGE_EDGE)
                        .map_err(|_| "Could not be decoded".to_owned())
                },
                move |this, result, _| {
                    // A read from before a sign-in change is dropped with its pixels.
                    if this.issues.image_generation != generation {
                        return false;
                    }
                    this.issues.image_reads = this.issues.image_reads.saturating_sub(1);
                    if !this.issues.images.contains_key(&url) {
                        return true;
                    }
                    let visible = this
                        .issues
                        .text
                        .as_ref()
                        .map(|text| text.linear_images())
                        .unwrap_or_default();
                    match result {
                        Ok((pixels, bytes)) => {
                            this.issues.admit_image(url, pixels, bytes, &visible)
                        }
                        Err(reason) => {
                            this.issues.images.insert(url, Picture::Failed(reason));
                        }
                    }
                    true
                },
                cx,
            );
        }
    }

    fn decoded_bytes(&self) -> usize {
        self.issues.decoded_bytes()
    }

    fn evict_images(&mut self, visible: &[String]) {
        self.issues.evict_images(visible, 0);
    }

    /// What is known of an image in the shown issue, for drawing it.
    fn picture(&self, url: &str) -> Picture {
        if !self.issues.view.images {
            return Picture::Elsewhere;
        }
        match self.issues.images.get(url) {
            Some(picture) => picture.clone(),
            // Not started because the shown issue's images fill the bound.
            None if self.decoded_bytes() >= IMAGE_MEMORY => Picture::Elsewhere,
            None => Picture::Loading,
        }
    }

    pub(super) fn open_new_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issues.view.new_form.is_none() && self.drafts_full() {
            cx.notify();
            return;
        }
        if self
            .issues
            .view
            .start_new(plait_client::host_state::request_id())
        {
            if !self.issues.form_fields.contains_key(&FormSlot::New) {
                self.make_form_fields(FormSlot::New, "", "", cx);
            }
            self.focus_form_title(&FormSlot::New, window, cx);
            self.persist_forms(cx);
        }
        cx.notify();
    }

    /// Edit brings back an unsaved edit, or reads the issue's exact text from
    /// Linear first: the details are shortened for display and must not be
    /// written back.
    pub(super) fn edit_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issues.view.show_edit() {
            if let Some(slot) = self.issues.view.form().map(IssueForm::slot) {
                self.focus_form_title(&slot, window, cx);
            }
            cx.notify();
            return;
        }
        let Some(issue_id) = self.issues.view.selected.clone() else {
            return;
        };
        if self.issues.reading_text.is_some() || self.drafts_full() {
            cx.notify();
            return;
        }
        self.issues.reading_text = Some(issue_id.clone());
        cx.notify();
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let read = issue_id.clone();
        let task = cx.background_executor().spawn(async move {
            linear::get_issue_text(&client, read).map_err(|error| linear::classify(&error, false))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.host.connection.epoch != epoch
                    || this.issues.reading_text.as_deref() != Some(issue_id.as_str())
                {
                    return;
                }
                this.issues.reading_text = None;
                match result {
                    Ok(text) => this.open_edit(&text, window, cx),
                    Err(error) => {
                        this.issues.edit_note = Some((
                            issue_id.clone(),
                            format!(
                                "The issue could not be read for editing. {}",
                                linear_settings::message(linear_settings::Pending::Loading, error)
                            ),
                        ));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Why Edit is not offered for this issue, if it is not.
    fn edit_note(&self, issue_id: &str) -> Option<String> {
        if self.issues.view.not_editable.as_deref() == Some(issue_id) {
            return Some(
                "This issue's title or description is too long for Plait to edit without losing text. Edit it in Linear."
                    .into(),
            );
        }
        self.issues
            .edit_note
            .as_ref()
            .filter(|(id, _)| id == issue_id)
            .map(|(_, note)| note.clone())
    }

    /// Opens an edit from the exact text the host read.
    pub(super) fn open_edit(
        &mut self,
        text: &linear::IssueText,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.issues.edit_note = None;
        if !self.issues.view.edits.contains_key(&text.issue_id) && self.drafts_full() {
            cx.notify();
            return;
        }
        if self
            .issues
            .view
            .begin_edit(text, plait_client::host_state::request_id())
            && let Some(form) = self.issues.view.form().cloned()
        {
            if !self.issues.form_fields.contains_key(&form.slot())
                && let FormKind::Edit {
                    title, description, ..
                } = &form.kind
            {
                self.make_form_fields(form.slot(), title, description, cx);
            }
            self.focus_form_title(&form.slot(), window, cx);
            self.persist_forms(cx);
        }
        cx.notify();
    }

    /// Whether the saved drafts are at their limit. Another form is refused
    /// with a note, so no unsaved text is ever dropped to make room.
    fn drafts_full(&mut self) -> bool {
        let full = self.issues.drafts.as_ref().is_some_and(Drafts::full);
        self.issues.drafts_full = full;
        full
    }

    fn focus_form_title(&self, slot: &FormSlot, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(fields) = self.issues.form_fields.get(slot) {
            window.focus(&fields.title.focus_handle(cx), cx);
        }
    }

    fn make_form_fields(
        &mut self,
        slot: FormSlot,
        title: &str,
        description: &str,
        cx: &mut Context<Self>,
    ) {
        let title_field = cx.new(|cx| {
            let mut field = TextField::new("Issue title", "Title", cx);
            field.set_text(title, cx);
            field
        });
        let text = description.to_owned();
        let editor = cx.new(|cx| {
            let mut editor = Composer::detached(cx);
            // A new composer waits for a thread; this one is ready at once.
            editor.set_enabled(true, None, cx);
            editor.set_purpose("Issue description", "Describe the issue. Markdown works.");
            editor.set_plain("Linear issues take text here. Add images in Linear.");
            editor.restore_draft(text, 0, None, cx);
            editor
        });
        let submit = cx.subscribe(&editor, |this, _, event: &ComposerEvent, cx| {
            if matches!(event, ComposerEvent::Submit { .. }) {
                this.save_issue_form(cx);
            }
        });
        // Typing in either field is kept on disk once it pauses.
        let typed = cx.observe(&editor, |this, _, cx| this.persist_forms(cx));
        let titled = cx.subscribe(&title_field, |this, _, _: &TextFieldEvent, cx| {
            this.persist_forms(cx)
        });
        self.issues.form_fields.insert(
            slot,
            FormFields {
                title: title_field,
                description: editor,
                _subscriptions: [submit, typed, titled],
            },
        );
    }

    fn cancel_issue_form(&mut self, cx: &mut Context<Self>) {
        if let Some(slot) = self.issues.view.cancel_form() {
            self.issues.form_fields.remove(&slot);
            self.persist_forms(cx);
        }
        cx.notify();
    }

    /// Reads the saved forms once, off the UI thread.
    fn load_saved_forms(&mut self, cx: &mut Context<Self>) {
        if self.issues.drafts.is_some() || self.issues.drafts_loading {
            return;
        }
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        self.issues.drafts_loading = true;
        let task = cx.background_executor().spawn(async move {
            journal
                .read_record::<Drafts>(linear_drafts::RECORD, linear_drafts::RECORD_LIMIT)
                .ok()
                .flatten()
                .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let drafts = task.await;
            let _ = this.update(cx, |this, cx| {
                this.issues.drafts_loading = false;
                this.issues.drafts = Some(drafts);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reopens this project's saved forms, once each time the view starts
    /// over for a project, behind whatever is shown. A host that cannot write
    /// issues reopens none, and the drafts stay on disk for one that can.
    fn restore_saved_forms(&mut self, cx: &mut Context<Self>) {
        let Some(checkout) = self.issues.view.checkout.clone() else {
            return;
        };
        let Some(drafts) = &self.issues.drafts else {
            return;
        };
        if self.issues.restored_for.as_ref() == Some(&checkout) || !self.issues.view.issue_writes {
            return;
        }
        let saved: Vec<_> = drafts.of(&checkout).cloned().collect();
        self.issues.restored_for = Some(checkout);
        for form in saved {
            if self
                .issues
                .view
                .restore_form(form.form_kind(), form.request_id.clone())
            {
                self.make_form_fields(form.slot(), &form.title, &form.description, cx);
            }
        }
    }

    /// This project's open forms as typed, folded into the saved drafts.
    /// Returns the drafts to write when they changed, or `None`.
    fn capture_forms(&mut self, cx: &mut Context<Self>) -> Option<Drafts> {
        let checkout = self.issues.view.checkout.clone()?;
        // Until the saved forms are read, writing would forget other projects'.
        if self.issues.drafts.is_none() || self.issues.restored_for.as_ref() != Some(&checkout) {
            return None;
        }
        let mut forms: Vec<_> = self
            .issues
            .view
            .forms()
            .filter_map(|form| {
                let fields = self.issues.form_fields.get(&form.slot())?;
                Some(SavedForm::new(
                    &checkout,
                    &form.kind,
                    fields.title.read(cx).text(),
                    fields.description.read(cx).saved_draft().0,
                    &form.request_id,
                ))
            })
            .collect();
        forms.sort_by_key(|form| match form.slot() {
            FormSlot::New => String::new(),
            FormSlot::Edit(id) => id,
        });
        let drafts = self.issues.drafts.as_mut()?;
        let changed = drafts.replace(&checkout, forms);
        if changed {
            self.issues.drafts_revision += 1;
        }
        changed.then(|| drafts.clone())
    }

    /// The drafts as typed now, with their revision and the writer, for a
    /// write that must not wait: Save, close and quit.
    pub(super) fn draft_flush(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(Arc<linear_drafts::Writer>, u64, Drafts)> {
        self.capture_forms(cx);
        let drafts = self.issues.drafts.clone()?;
        Some((
            self.issues.drafts_writer.clone(),
            self.issues.drafts_revision,
            drafts,
        ))
    }

    /// Records this project's open forms as typed, and writes them after
    /// typing pauses. Writing runs on the background executor.
    fn persist_forms(&mut self, cx: &mut Context<Self>) {
        if self.capture_forms(cx).is_none() {
            return;
        }
        let revision = self.issues.drafts_revision;
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_PAUSE).await;
            // Typing went on: a later timer writes the newer text.
            let Ok(Some((writer, drafts))) = this.update(cx, |this, _| {
                (this.issues.drafts_revision == revision)
                    .then(|| this.issues.drafts.clone())
                    .flatten()
                    .map(|drafts| (this.issues.drafts_writer.clone(), drafts))
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move {
                    let _ = writer.write(&journal, revision, &drafts);
                })
                .await;
        })
        .detach();
    }

    /// After a conflict, keeps the person's text and bases it on Linear's version.
    fn keep_my_edit(&mut self, cx: &mut Context<Self>) {
        if self.issues.view.keep_mine() {
            self.persist_forms(cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn save_issue_form_for_scene(&mut self, cx: &mut Context<Self>) {
        self.save_issue_form(cx);
    }

    /// Writes the form in front. Only this explicit action sends anything,
    /// and only once its draft, with the request's ID, is on disk: a save
    /// after a crash then names the same request.
    fn save_issue_form(&mut self, cx: &mut Context<Self>) {
        let Some(slot) = self.issues.view.form().map(|f| f.slot()) else {
            return;
        };
        let Some(fields) = self.issues.form_fields.get(&slot) else {
            return;
        };
        let typed = fields.title.read(cx).text().to_owned();
        // The field shows the title on one line; one the person did not change
        // is Linear's exactly, so it is not rewritten in its normalized form.
        let title = match self.issues.view.form().map(|f| &f.kind) {
            Some(FormKind::Edit { title: base, .. })
                if super::text_field::single_line(base) == typed =>
            {
                base.clone()
            }
            _ => typed,
        };
        let description = fields.description.read(cx).saved_draft().0.to_owned();
        let fresh = plait_client::host_state::request_id();
        let request = match self.issues.view.submit_form(&title, &description, fresh) {
            Submit::Send(request) => request,
            Submit::Closed(slot) => {
                self.issues.form_fields.remove(&slot);
                self.persist_forms(cx);
                cx.notify();
                return;
            }
            Submit::Nothing => {
                cx.notify();
                return;
            }
        };
        cx.notify();
        if cfg!(test) {
            return;
        }
        let drafts = self.draft_flush(cx);
        let journal = self.session.journal.clone();
        let flush = cx.background_executor().spawn(async move {
            let (Some(journal), Some((writer, revision, drafts))) = (journal, drafts) else {
                return Err("The desktop's saved state is unavailable.".to_owned());
            };
            writer.write(&journal, revision, &drafts)
        });
        // The machine Save was pressed on: it alone may receive the request.
        let target = (self.host.connection.epoch, self.tabs.host.clone());
        cx.spawn(async move |this, cx| {
            let flushed = flush.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_flush(request, target, flushed, cx);
            });
        })
        .detach();
    }

    /// After the draft is on disk: sends the request only to the machine and
    /// connection it was saved on. Returns whether it was sent.
    pub(super) fn finish_flush(
        &mut self,
        request: linear_issues::FormRequest,
        target: (u64, plait_client::tabs::HostKey),
        flushed: Result<(), String>,
        cx: &mut Context<Self>,
    ) -> bool {
        cx.notify();
        if self.host.connection.epoch != target.0 || self.tabs.host != target.1 {
            self.issues.view.not_sent(
                &request,
                "The machine changed before this was sent. Nothing was sent; save again to send it here.",
            );
            return false;
        }
        if flushed.is_err() {
            self.issues.view.draft_unsaved(&request);
            return false;
        }
        if !cfg!(test) {
            self.send_issue_form(request, cx);
        }
        true
    }

    fn send_issue_form(&mut self, request: linear_issues::FormRequest, cx: &mut Context<Self>) {
        let call = request.clone();
        self.linear_task(
            move |client| {
                let result = match call.kind {
                    FormKind::New { team_id, .. } => linear::create_issue(
                        client,
                        team_id,
                        call.title.unwrap_or_default(),
                        call.description,
                        None,
                        call.request_id,
                    ),
                    FormKind::Edit {
                        issue_id,
                        expected_updated_at,
                        ..
                    } => linear::update_issue(
                        client,
                        issue_id,
                        expected_updated_at,
                        call.title,
                        call.description,
                        call.request_id,
                    ),
                };
                result.map_err(|error| linear::write_failure(&error))
            },
            move |this, result, cx| {
                match this.issues.view.form_reply(&request, result) {
                    FormDone::Created(id) => {
                        this.issues.form_fields.remove(&FormSlot::New);
                        this.select_issue(&id, cx);
                        this.load_issue_list(true, cx);
                    }
                    FormDone::Closed(slot) => {
                        this.issues.form_fields.remove(&slot);
                        this.load_issue_list(true, cx);
                    }
                    FormDone::Reload => {
                        // Linear's version, beside a refused edit or after an accepted one.
                        if let FormKind::Edit { issue_id, .. } = &request.kind
                            && this.issues.view.selected.as_deref() == Some(issue_id)
                            && let Some((id, generation)) = this.issues.view.reload_detail()
                        {
                            this.load_issue_detail(id, generation, cx);
                        }
                        // An accepted edit closed its form; a refused one is still open.
                        if let FormKind::Edit { issue_id, .. } = &request.kind
                            && !this.issues.view.edits.contains_key(issue_id)
                        {
                            this.load_issue_list(true, cx);
                        }
                    }
                    FormDone::Nothing => {}
                }
                this.persist_forms(cx);
                true
            },
            cx,
        );
    }

    /// The form in front: a title, a description, and its outcome.
    fn issue_form(&self, cx: &mut Context<Self>) -> Option<Div> {
        let form = self.issues.view.form()?;
        let fields = self.issues.form_fields.get(&form.slot())?;
        let shown = self.issues.view.detail.as_ref().map(|d| &d.issue);
        // A new issue names the team it was written for, which Save sends it to.
        let elsewhere = match &form.kind {
            FormKind::New { team_id, team_name } => self
                .issues
                .view
                .team()
                .is_none_or(|b| &b.team_id != team_id)
                .then(|| team_name.clone().unwrap_or_else(|| "its team".into())),
            FormKind::Edit { .. } => None,
        };
        let heading = match &form.kind {
            FormKind::New { team_name, .. } => format!(
                "New issue in {}",
                team_name.clone().unwrap_or_else(|| "its team".into())
            ),
            FormKind::Edit { .. } => format!(
                "Edit {}",
                shown
                    .map(|d| d.summary.identifier.clone())
                    .unwrap_or_default()
            ),
        };
        let saving = form.frozen();
        let changed = form.status == FormStatus::Changed;
        // While a save is in flight the text is shown, not edited.
        let frozen_text = |id: &'static str, text: String| {
            div()
                .id(id)
                .w_full()
                .min_w_0()
                .px_3()
                .py_2()
                .rounded(px(ITEM_CORNER))
                .border_1()
                .border_color(rgb(colors().border))
                .text_size(px(14.))
                .line_height(px(22.))
                .text_color(rgb(colors().muted))
                .children(
                    text.lines()
                        .map(|line| div().min_h(px(22.)).child(line.to_owned()))
                        .collect::<Vec<_>>(),
                )
        };
        let note = match &form.status {
            FormStatus::Failed { note, .. } => Some(note.clone()),
            FormStatus::Unknown => {
                Some("Linear may have saved this. Check in Linear before saving again.".to_owned())
            }
            _ => None,
        };
        let label = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors().muted))
                .child(text)
        };
        Some(
            column()
                .w_full()
                .max_w(px(READING_WIDTH))
                .gap_3()
                .child(
                    div()
                        .text_size(px(20.))
                        .line_height(px(28.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(colors().text))
                        .child(heading),
                )
                .when_some(elsewhere, |el, team| {
                    el.child(
                        tag(
                            "issue-form-team",
                            format!(
                                "This draft is for {team}, not this project's linked team. Save sends it to {team}; Discard drops it."
                            ),
                            colors().amber,
                        )
                        .line_height(px(18.)),
                    )
                })
                .when(changed, |el| el.child(self.linear_version(form, cx)))
                .child(label(
                    "issue-form-title-label",
                    if changed { "Your title" } else { "Title" },
                ))
                .child(
                    // Enter in the title goes on to the description; Cmd-Enter saves.
                    div()
                        .w_full()
                        .on_key_down(cx.listener({
                            let description = fields.description.clone();
                            move |this, event: &KeyDownEvent, window, cx| {
                                if event.keystroke.key != "enter" {
                                    return;
                                }
                                cx.stop_propagation();
                                if event.keystroke.modifiers.platform {
                                    this.save_issue_form(cx);
                                } else {
                                    window.focus(&description.focus_handle(cx), cx);
                                }
                            }
                        }))
                        .child(if saving {
                            frozen_text(
                                "issue-form-title-saving",
                                fields.title.read(cx).text().to_owned(),
                            )
                            .into_any_element()
                        } else {
                            fields.title.clone().into_any_element()
                        }),
                )
                .child(label(
                    "issue-form-description-label",
                    if changed {
                        "Your description"
                    } else {
                        "Description"
                    },
                ))
                .child(if saving {
                    frozen_text(
                        "issue-form-description-saving",
                        fields.description.read(cx).saved_draft().0.to_owned(),
                    )
                    .min_h(px(180.))
                    .into_any_element()
                } else {
                    div()
                        .w_full()
                        .min_h(px(180.))
                        .px_3()
                        .py_2()
                        .rounded(px(ITEM_CORNER))
                        .border_1()
                        .border_color(rgb(colors().border))
                        .bg(rgb(colors().panel))
                        .child(fields.description.clone())
                        .into_any_element()
                })
                .child(tag(
                    "issue-form-hint",
                    "Markdown works. Cmd-Enter saves; nothing is sent until you save.",
                    colors().subtle,
                ))
                .when_some(note, |el, note| {
                    el.child(tag("issue-form-note", note, colors().amber).line_height(px(18.)))
                })
                .when(!changed, |el| {
                    el.child(
                        row()
                            .gap_2()
                            .child(div().flex_1())
                            .child(
                                button("issue-form-cancel", "Discard")
                                    .disabled(saving)
                                    .on_activate(cx, |this, _, cx| this.cancel_issue_form(cx)),
                            )
                            .child(
                                toned_button(
                                    "issue-form-save",
                                    if saving { "Saving…" } else { "Save" },
                                    Tone::Primary,
                                )
                                .disabled(saving || form.status == FormStatus::Unknown)
                                .on_activate(cx, |this, _, cx| this.save_issue_form(cx)),
                            ),
                    )
                }),
        )
    }

    /// Linear's version of an issue whose edit was refused, above the person's,
    /// with the two ways forward.
    fn linear_version(&self, form: &IssueForm, cx: &mut Context<Self>) -> Div {
        let FormKind::Edit {
            issue_id,
            expected_updated_at,
            ..
        } = &form.kind
        else {
            return div();
        };
        // Linear's version is the re-read issue, once it is newer than the edit's start.
        let theirs = self
            .issues
            .view
            .detail
            .as_ref()
            .map(|d| &d.issue)
            .filter(|d| &d.summary.id == issue_id && &d.summary.updated_at != expected_updated_at);
        column()
            .gap_2()
            .p_3()
            .rounded(px(ITEM_CORNER))
            .border_1()
            .border_color(rgb(colors().amber))
            .child(
                tag(
                    "issue-conflict-note",
                    "Someone changed this issue in Linear after you started editing. Nothing was saved.",
                    colors().amber,
                )
                .line_height(px(18.)),
            )
            .child(match theirs {
                Some(theirs) => column()
                    .gap_1()
                    .child(tag("issue-conflict-label", "Linear's version", colors().muted))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(colors().text))
                            .child(theirs.summary.title.clone()),
                    )
                    .child(plain_text(
                        "issue-conflict-description",
                        theirs.description.as_deref().unwrap_or("No description."),
                    )),
                None => column().child(tag(
                    "issue-conflict-loading",
                    "Reading Linear's version…",
                    colors().muted,
                )),
            })
            .child(
                row()
                    .gap_2()
                    .child(div().flex_1())
                    .child(
                        button("issue-conflict-discard", "Discard mine")
                            .on_activate(cx, |this, _, cx| this.cancel_issue_form(cx)),
                    )
                    .child(
                        toned_button("issue-conflict-keep", "Keep editing mine", Tone::Filled)
                            .disabled(theirs.is_none())
                            .on_activate(cx, |this, _, cx| this.keep_my_edit(cx)),
                    ),
            )
    }
    fn close_issue(&mut self, cx: &mut Context<Self>) {
        self.issues.view.clear_selection();
        self.save_issues_bookmark(cx);
        cx.notify();
    }

    /// The team's workflow states, to move the shown issue to one of them.
    fn open_state_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = &self.issues.view;
        let (Some(states), Some(detail)) = (&view.states, &view.detail) else {
            return;
        };
        let current = detail.issue.summary.state.id.clone();
        let items = states
            .iter()
            .map(|state| {
                let item = super::palette::Item::new(
                    format!("issue-state:{}", state.id),
                    state.name.clone(),
                    super::commands::Command::IssueState(state.id.clone()),
                )
                .detail(state_kind(&state.kind))
                .section("Workflow states");
                if state.id == current {
                    item.badge("Current", colors().subtle).enabled(false)
                } else {
                    item
                }
            })
            .collect::<Vec<_>>();
        let options = super::palette::Options {
            name: "Change the issue's status".into(),
            placeholder: "Search states…".into(),
            empty: "This team has no other states.".into(),
            initial: None,
            mark: None,
            aside: None,
            note: Some("Moves the issue in Linear for everyone on the team.".into()),
        };
        let palette = cx.new(|cx| super::palette::Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }

    /// A person chose a state: send it once. The host records the request
    /// before sending and never resends one whose outcome is unknown.
    pub(super) fn change_issue_state(&mut self, state_id: &str, cx: &mut Context<Self>) {
        let request = plait_client::host_state::request_id();
        if let Some(change) = self.issues.view.start_state_change(state_id, request) {
            self.send_state_change(change, cx);
        }
        cx.notify();
    }

    fn retry_state_change(&mut self, cx: &mut Context<Self>) {
        if let Some(change) = self.issues.view.retry_state_change() {
            self.send_state_change(change, cx);
        }
        cx.notify();
    }

    fn send_state_change(&mut self, change: linear_issues::StateChange, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let call = change.clone();
        self.linear_task(
            move |client| {
                linear::update_issue_state(client, call.issue_id, call.state_id, call.request_id)
                    .map_err(|error| linear::write_failure(&error))
            },
            move |this, result, cx| {
                let accepted = result.is_ok();
                if this.issues.view.state_reply(&change, result)
                    && let Some((id, generation)) = this.issues.view.reload_detail()
                {
                    this.load_issue_detail(id, generation, cx);
                }
                // The list shows each issue's state; read it again once moved.
                if accepted {
                    this.load_issue_list(true, cx);
                }
                true
            },
            cx,
        );
    }

    fn refresh_issues(&mut self, cx: &mut Context<Self>) {
        self.load_issue_list(true, cx);
        if let Some((id, generation)) = self.issues.view.reload_detail() {
            self.load_issue_detail(id, generation, cx);
        }
        cx.notify();
    }

    /// A pasted identifier or Linear URL opens that issue directly. Title
    /// search is broad and never picks an issue by itself.
    fn resolve_issue_reference(&mut self, text: &str, cx: &mut Context<Self>) {
        let reference = text.trim().to_owned();
        if cfg!(test) || !looks_like_reference(&reference) {
            return;
        }
        let checkout = self.visible_project_checkout().to_owned();
        let epoch_checkout = checkout.clone();
        self.linear_task(
            move |client| {
                linear::resolve_issue(client, checkout, reference, false)
                    .map_err(|error| linear::classify(&error, false))
            },
            move |this, result, cx| {
                // Only for the project still shown; a miss leaves the search alone.
                if this.issues.view.checkout.as_deref() != Some(epoch_checkout.as_path()) {
                    return false;
                }
                let Ok(reply) = result else { return false };
                let id = reply.issue.id.clone();
                if let Some(generation) = this.issues.view.select(&id) {
                    this.save_issues_bookmark(cx);
                    this.load_issue_detail(id, generation, cx);
                }
                true
            },
            cx,
        );
    }
}

/// `PLA-142`, or an https://linear.app/…/issue/PLA-142 address.
fn looks_like_reference(text: &str) -> bool {
    let id = text
        .strip_prefix("https://linear.app/")
        .and_then(|rest| rest.split('/').nth(2))
        .unwrap_or(text);
    let Some((team, number)) = id.split_once('-') else {
        return false;
    };
    (1..=16).contains(&team.len())
        && team.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && team.chars().all(|c| c.is_ascii_alphanumeric())
        && (1..=9).contains(&number.len())
        && number.chars().all(|c| c.is_ascii_digit())
}

/// What the Issues tab shows instead of issues, if anything.
enum Blocked {
    Offline,
    Checking,
    OlderHost,
    SignedOut,
    Unlinked,
}

impl Plait {
    fn issues_blocked(&self) -> Option<Blocked> {
        if !self.host.connection.connected {
            return Some(Blocked::Offline);
        }
        match self.linear.account.auth {
            Auth::Loading => return Some(Blocked::Checking),
            Auth::OlderHost => return Some(Blocked::OlderHost),
            Auth::SignedOut | Auth::KeyRejected => return Some(Blocked::SignedOut),
            Auth::SignedIn { .. } => {}
        }
        match &self.issues.view.binding {
            None => Some(Blocked::Checking),
            Some(None) => Some(Blocked::Unlinked),
            Some(Some(_)) => None,
        }
    }

    fn issues_blocked_view(&self, blocked: Blocked, cx: &mut Context<Self>) -> Div {
        let checkout = self.visible_project_checkout().to_owned();
        let (text, action): (&str, Option<(&str, bool)>) = match blocked {
            Blocked::Offline => (
                "Connect to the host to see this project's Linear issues.",
                None,
            ),
            Blocked::Checking => ("Checking Linear…", None),
            Blocked::OlderHost => (
                "Linear needs a newer host. Update Plait's host to use it.",
                None,
            ),
            Blocked::SignedOut => (
                "Sign in to Linear to see this project's issues.",
                Some(("Linear settings…", true)),
            ),
            Blocked::Unlinked => (
                "Link this project to a Linear team to see its issues.",
                Some(("Project settings…", false)),
            ),
        };
        column()
            .gap_3()
            .p_4()
            .child(tag("issues-blocked", text, colors().muted).line_height(px(18.)))
            .when_some(action, |el, (label, account)| {
                el.child(
                    toned_button("issues-blocked-action", label, Tone::Filled).on_activate(
                        cx,
                        move |this, window, cx| {
                            if account {
                                this.open_page(
                                    Some(settings::SettingsPage::Application),
                                    window,
                                    cx,
                                );
                                this.settings_tab = settings::SettingsTab::Linear;
                            } else {
                                this.show_settings_for_project(checkout.clone(), window, cx);
                            }
                            cx.notify();
                        },
                    ),
                )
            })
    }

    fn issue_row(&self, index: usize, issue: &IssueSummary, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.issues.view.selected.as_deref() == Some(issue.id.as_str());
        let id = issue.id.clone();
        nav_row(
            ("issue-row", index),
            format!("{} {}, {}", issue.identifier, issue.title, issue.state.name),
            selected,
        )
        .aria_selected(selected)
        .gap_2()
        .child(
            div()
                .w(px(72.))
                .flex_shrink_0()
                .truncate()
                .text_size(px(11.))
                .text_color(rgb(colors().subtle))
                .child(issue.identifier.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(issue.title.clone()),
        )
        .child(tag(
            ("issue-row-state", index),
            issue.state.name.clone(),
            colors().subtle,
        ))
        .on_activate(cx, move |this, _, cx| this.select_issue(&id, cx))
        .into_any_element()
    }

    fn issue_list(&self, cx: &mut Context<Self>) -> Div {
        let view = &self.issues.view;
        let mut filters = row().gap_1().flex_wrap();
        for (index, (filter, label)) in FILTERS.iter().enumerate() {
            let selected = view.filter == *filter;
            let filter = *filter;
            filters = filters.child(
                toned_button(
                    ("issues-filter", index),
                    *label,
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .role(Role::Tab)
                .aria_selected(selected)
                .on_activate(cx, move |this, _, cx| {
                    if this.issues.view.set_filter(filter) {
                        this.load_issue_list(false, cx);
                        cx.notify();
                    }
                }),
            );
        }
        let rows = view
            .issues
            .iter()
            .enumerate()
            .map(|(index, issue)| self.issue_row(index, issue, cx))
            .collect::<Vec<_>>();
        let empty = view.issues.is_empty() && view.list == Load::Idle && view.cache.is_some();
        column()
            .gap_2()
            .min_h_0()
            .flex_1()
            .children(self.issues.search.clone())
            .child(filters)
            .child(
                column()
                    .id("issue-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_px()
                    .children(rows)
                    .when(empty, |el| {
                        el.child(tag(
                            "issues-empty",
                            if view.search.is_empty() {
                                "No issues match this filter."
                            } else {
                                "No issue titles match this search."
                            },
                            colors().muted,
                        ))
                    })
                    .when(view.has_more, |el| {
                        el.child(
                            button("issues-more", "Load more")
                                .disabled(view.list == Load::Loading)
                                .on_activate(cx, |this, _, cx| this.load_more_issues(cx)),
                        )
                    }),
            )
            .when(view.list == Load::Loading, |el| {
                el.child(tag("issues-loading", "Loading issues…", colors().muted))
            })
            .when_some(view.list_note.clone(), |el, note| {
                el.child(tag("issues-note", note, colors().amber).line_height(px(18.)))
            })
            .when_some(
                view.cache.as_ref().and_then(linear_issues::freshness),
                |el, note| {
                    el.child(tag("issues-freshness", note, colors().amber).line_height(px(18.)))
                },
            )
    }

    fn issue_detail(&self, detail: &IssueDetail, wide: bool, cx: &mut Context<Self>) -> Div {
        let issue = &detail.summary;
        let text = self
            .issues
            .text
            .as_ref()
            .filter(|text| text.matches(detail));
        column()
            .w_full()
            .max_w(px(READING_WIDTH))
            .gap_4()
            .child(self.issue_actions(issue, wide, cx))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors().text))
                    .child(issue.title.clone()),
            )
            .child(self.issue_facts(issue, cx))
            .when_some(self.edit_note(&issue.id), |el, note| {
                el.child(tag("issue-edit-note", note, colors().amber).line_height(px(18.)))
            })
            .children(self.state_change_note(cx))
            .child(issue_description(detail, text, &|url| self.picture(url)))
            .when(!detail.attachments.is_empty(), |el| {
                el.child(
                    tag(
                        "issue-attachments",
                        format!(
                            "Attachments, open in Linear: {}",
                            detail
                                .attachments
                                .iter()
                                .map(|a| a.title.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        colors().subtle,
                    )
                    .line_height(px(18.)),
                )
            })
            .when(!detail.comments.is_empty(), |el| {
                el.child(issue_comments(detail, text, &|url| self.picture(url)))
            })
    }

    /// The issue's identifier and what can be done with it.
    fn issue_actions(&self, issue: &IssueSummary, wide: bool, cx: &mut Context<Self>) -> Div {
        let url = issue.url.clone();
        row()
            .gap_2()
            .w_full()
            .when(!wide, |el| {
                el.child(
                    button("issue-back", "All issues")
                        .on_activate(cx, |this, _, cx| this.close_issue(cx)),
                )
            })
            .child(tag(
                "issue-identifier",
                issue.identifier.clone(),
                colors().subtle,
            ))
            .child(div().flex_1())
            .when(
                self.issues.view.issue_writes
                    && self.issues.view.not_editable.as_deref() != Some(issue.id.as_str()),
                |el| {
                    el.child(
                        button(
                            "issue-edit",
                            if self.issues.reading_text.is_some() {
                                "Reading…"
                            } else {
                                "Edit"
                            },
                        )
                        .disabled(self.issues.reading_text.is_some())
                        .on_activate(cx, |this, window, cx| this.edit_issue(window, cx)),
                    )
                },
            )
            .child(
                button("issue-open-linear", "Open in Linear")
                    .on_activate(cx, move |_, _, cx| cx.open_url(&url)),
            )
            .child(
                button("issue-add-to-thread", "Add to thread…")
                    .on_activate(cx, |this, window, cx| {
                        this.choose_thread_for_issue(window, cx)
                    }),
            )
            .child(
                toned_button("issue-start-thread", "Start thread", Tone::Primary)
                    .on_activate(cx, |this, window, cx| {
                        this.start_thread_from_issue(window, cx)
                    }),
            )
    }

    /// Status, priority, assignee and labels. Status is a menu when the host
    /// can record the change and the team's states are known.
    fn issue_facts(&self, issue: &IssueSummary, cx: &mut Context<Self>) -> Div {
        let view = &self.issues.view;
        let sending = view
            .state_change
            .as_ref()
            .filter(|c| c.issue_id == issue.id && c.status == linear_issues::ChangeStatus::Sending);
        let status = match (view.writes, &view.states, sending) {
            (_, _, Some(change)) => super::permissions::menu_chip(
                "issue-status",
                Some("Status"),
                None,
                format!("Moving to {}…", change.state_name).into(),
                "".into(),
                Some("Waiting for Linear."),
            )
            .into_any_element(),
            (Some(true), Some(_), None) => super::permissions::menu_chip(
                "issue-status",
                Some("Status"),
                None,
                issue.state.name.clone().into(),
                "Move the issue to another state in Linear".into(),
                None,
            )
            .on_activate(cx, |this, window, cx| this.open_state_menu(window, cx))
            .into_any_element(),
            _ => tag("issue-status", issue.state.name.clone(), colors().text).into_any_element(),
        };
        let mut facts = vec![linear_issues::priority(issue.priority).to_owned()];
        if let Some(assignee) = &issue.assignee {
            facts.push(assignee.name.clone());
        }
        row()
            .gap_2()
            .flex_wrap()
            .w_full()
            .child(status)
            .child(tag("issue-facts", facts.join(" · "), colors().muted))
            .children(issue.labels.iter().enumerate().map(|(index, label)| {
                tag(("issue-label", index), label.name.clone(), colors().muted)
                    .px_2()
                    .rounded(px(ITEM_CORNER))
                    .border_1()
                    .border_color(rgb(colors().border))
            }))
    }

    /// Why the last status change did not happen, inline beneath the status.
    fn state_change_note(&self, cx: &mut Context<Self>) -> Option<Div> {
        let change = self.issues.view.state_change.as_ref()?;
        if self.issues.view.detail.as_ref()?.issue.summary.id != change.issue_id {
            return None;
        }
        let (text, retry) = match &change.status {
            linear_issues::ChangeStatus::Sending => return None,
            linear_issues::ChangeStatus::Failed { note, retry } => (note.clone(), *retry),
            linear_issues::ChangeStatus::Unknown => (
                format!(
                    "Linear may have moved it to {}. Check in Linear before choosing again.",
                    change.state_name
                ),
                false,
            ),
        };
        Some(
            row()
                .gap_2()
                .w_full()
                .child(
                    tag("issue-status-note", text, colors().amber)
                        .flex_1()
                        .min_w_0()
                        .line_height(px(18.)),
                )
                .when(retry, |el| {
                    el.child(
                        button("issue-status-retry", "Try again")
                            .on_activate(cx, |this, _, cx| this.retry_state_change(cx)),
                    )
                })
                .child(
                    button("issue-status-dismiss", "Dismiss").on_activate(cx, |this, _, cx| {
                        this.issues.view.dismiss_state_change();
                        cx.notify();
                    }),
                ),
        )
    }

    fn issue_detail_pane(&self, wide: bool, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let view = &self.issues.view;
        let form = self.issue_form(cx);
        if form.is_none() {
            view.selected.as_ref()?;
        }
        let body = match (&view.detail, view.detail_load) {
            _ if form.is_some() => form.unwrap_or_else(column),
            (Some(reply), _) => self
                .issue_detail(&reply.issue, wide, cx)
                .when_some(linear_issues::freshness(&reply.cache), |el, note| {
                    el.child(tag("issue-freshness", note, colors().amber))
                }),
            (None, Load::Failed) => column().gap_2().child(
                tag(
                    "issue-detail-note",
                    view.detail_note.clone().unwrap_or_default(),
                    colors().amber,
                )
                .line_height(px(18.)),
            ),
            (None, _) => column().child(tag(
                "issue-detail-loading",
                "Loading the issue…",
                colors().muted,
            )),
        };
        // Its own scroll, bounded by the tab, so long text never runs past it.
        Some(
            div()
                .id("issue-detail")
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .overflow_y_scroll()
                .px_2()
                .pb_6()
                .child(body),
        )
    }

    pub(super) fn issues_tab(&self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let team = self.issues.view.team().map(|binding| {
            format!(
                "{}{}",
                binding.team_name.as_deref().unwrap_or("Linked team"),
                binding
                    .team_key
                    .as_deref()
                    .map(|key| format!(" ({key})"))
                    .unwrap_or_default()
            )
        });
        // The sheet's header already names the tab; this row says which team.
        let header = row()
            .gap_2()
            .px_4()
            .py_2()
            .when_some(team, |el, team| {
                el.child(tag("issues-team", team, colors().muted))
            })
            .when(self.issues.drafts_full, |el| {
                el.child(
                    tag(
                        "issues-drafts-full",
                        "64 unsaved issue drafts are kept. Save or discard one to start another.",
                        colors().amber,
                    )
                    .line_height(px(18.)),
                )
            })
            .child(div().flex_1())
            .when(
                self.issues.view.issue_writes
                    && self.issues.view.team().is_some()
                    && !self.issues.view.composing_new,
                |el| {
                    el.child(
                        button("issues-new", "New issue")
                            .on_activate(cx, |this, window, cx| this.open_new_issue(window, cx)),
                    )
                },
            )
            .child(
                button("issues-refresh", "Refresh")
                    .disabled(self.issues_blocked().is_some())
                    .on_activate(cx, |this, _, cx| this.refresh_issues(cx)),
            );
        let body = if let Some(blocked) = self.issues_blocked() {
            self.issues_blocked_view(blocked, cx)
        } else {
            let detail = self.issue_detail_pane(wide, cx);
            match (wide, detail) {
                (true, detail) => row()
                    .items_stretch()
                    .gap_6()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .child(
                        div()
                            .w(px(420.))
                            .flex_shrink_0()
                            .flex()
                            .min_h_0()
                            .child(self.issue_list(cx)),
                    )
                    .children(detail),
                // Narrow: the selected issue takes the whole width, with a way back.
                (false, Some(detail)) => {
                    div().flex().flex_1().min_h_0().px_4().pb_4().child(detail)
                }
                (false, None) => div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .child(self.issue_list(cx)),
            }
        };
        column()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// The description as Markdown once parsed; as text until then.
fn issue_description(
    detail: &IssueDetail,
    text: Option<&Arc<IssueText>>,
    picture: &dyn Fn(&str) -> Picture,
) -> AnyElement {
    match (text, detail.description.as_deref()) {
        (Some(text), _) if !text.description.is_empty() => {
            issue_text::blocks(&text.description, "issue-description", picture).into_any_element()
        }
        (None, Some(source)) if !source.trim().is_empty() => {
            plain_text("issue-description", source).into_any_element()
        }
        _ => tag("issue-description", "No description.", colors().muted).into_any_element(),
    }
}

fn issue_comments(
    detail: &IssueDetail,
    text: Option<&Arc<IssueText>>,
    picture: &dyn Fn(&str) -> Picture,
) -> Div {
    column()
        .gap_3()
        .pt_4()
        .border_t_1()
        .border_color(rgb(colors().border))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(colors().muted))
                .child("Comments"),
        )
        .children(detail.comments.iter().enumerate().map(|(index, comment)| {
            let key = format!("issue-comment-{index}");
            let body = match text.and_then(|text| text.comments.get(index)) {
                Some(blocks) => issue_text::blocks(blocks, &key, picture).into_any_element(),
                None => plain_text(("issue-comment", index), &comment.body).into_any_element(),
            };
            column()
                .gap_1()
                .child(tag(
                    ("issue-comment-author", index),
                    format!(
                        "{} · {}",
                        comment.author.as_deref().unwrap_or("Someone"),
                        comment.created_at.get(..10).unwrap_or(&comment.created_at)
                    ),
                    colors().subtle,
                ))
                .child(body)
        }))
        .when(detail.comments_has_more, |el| {
            el.child(tag(
                "issue-comments-more",
                "More comments are in Linear.",
                colors().subtle,
            ))
        })
}

/// Linear's text before it is parsed: paragraphs kept, nothing interpreted.
fn plain_text(id: impl Into<ElementId>, text: &str) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(rgb(colors().text))
        .children(
            text.lines()
                .map(|line| div().min_h(px(22.)).child(line.to_owned()))
                .collect::<Vec<_>>(),
        )
}

fn state_kind(kind: &str) -> &'static str {
    match kind {
        "triage" => "Triage",
        "backlog" => "Backlog",
        "unstarted" => "Not started",
        "started" => "In progress",
        "completed" => "Done",
        "canceled" => "Cancelled",
        _ => "",
    }
}

impl Plait {
    fn issue_reference(&self) -> Option<(IssueRef, String)> {
        let detail = &self.issues.view.detail.as_ref()?.issue;
        let workspace = self.issues.view.team().and_then(|b| b.workspace_id.clone());
        let issue = IssueRef::of(&detail.summary, workspace);
        let text = linear_issues::reference(&issue, detail.description.as_deref());
        Some((issue, text))
    }

    /// A new draft in this project, with the issue in it. Nothing is sent;
    /// the thread is linked to the issue once the host creates it.
    pub(super) fn start_thread_from_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((issue, text)) = self.issue_reference() else {
            return;
        };
        let project = self.visible_project_checkout().to_owned();
        self.open_draft_in(project, window, cx);
        let Some(draft) = self.draft.id.clone() else {
            return;
        };
        self.issues.pending_links.insert(draft.clone(), issue);
        self.add_reference(Destination::Draft(draft), text, window, cx);
    }

    /// The project's threads, to add the issue to one of their drafts.
    fn choose_thread_for_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.issue_reference() else {
            return;
        };
        let project = self.visible_project_checkout().to_owned();
        let items = self
            .host
            .connection
            .threads()
            .iter()
            .filter(|thread| {
                thread.archived != Some(true) && self.project_of(&thread.checkout) == project
            })
            .map(|thread| {
                super::palette::Item::new(
                    format!("issue-thread:{}", thread.id),
                    self.thread_title(&thread.id),
                    super::commands::Command::IssueToThread(thread.id.clone()),
                )
                .section("Threads in this project")
            })
            .collect::<Vec<_>>();
        self.issues.offer = Some(offer);
        let options = super::palette::Options {
            name: "Add the issue to a thread".into(),
            placeholder: "Search threads…".into(),
            empty: "This project has no threads yet. Start one from the issue instead.".into(),
            initial: None,
            mark: None,
            aside: None,
            note: Some("The issue is added to the thread's message box; nothing is sent.".into()),
        };
        let palette = cx.new(|cx| super::palette::Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }

    pub(super) fn add_issue_to_thread(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, text)) = self.issues.offer.take() else {
            return;
        };
        self.select_host_thread(id.clone(), window, cx);
        self.add_reference(Destination::Thread(id), text, window, cx);
    }

    /// Appends `text` to the destination's message box once it accepts input,
    /// trying briefly while the thread loads. Leaving it first drops the text.
    fn add_reference(
        &mut self,
        to: Destination,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..50 {
                let done = this
                    .update_in(cx, |this, window, cx| {
                        let composer = match &to {
                            Destination::Draft(id)
                                if this.draft.id.as_ref() == Some(id) && this.draft.open =>
                            {
                                this.draft.composer.clone()
                            }
                            Destination::Thread(id)
                                if this.host.thread.selected.as_ref() == Some(id) =>
                            {
                                this.workspace.composers.get(id).cloned()
                            }
                            // Somewhere else now: drop it rather than surprise another draft.
                            _ => return true,
                        };
                        composer.is_some_and(|composer| {
                            composer
                                .update(cx, |composer, cx| composer.append_block(&text, window, cx))
                        })
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }

    /// Links a thread created from an issue's draft. Called once the host
    /// confirms the thread; a failure leaves the draft's text reference.
    pub(super) fn link_created_thread(
        &mut self,
        draft: Option<&str>,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(issue) = draft.and_then(|draft| self.issues.pending_links.remove(draft)) else {
            return;
        };
        if cfg!(test) {
            return;
        }
        let thread = thread.to_owned();
        self.linear_task(
            move |client| {
                linear::link_thread(
                    client,
                    thread,
                    issue.workspace_id,
                    issue.issue_id,
                    issue.identifier,
                    issue.url,
                    issue.title,
                )
                .map_err(|error| linear::classify(&error, false))
            },
            |this, result, _| match result {
                Ok(reply) => {
                    this.issues
                        .links
                        .insert(reply.link.thread_id.clone(), reply.link);
                    true
                }
                Err(_) => false,
            },
            cx,
        );
    }

    /// Reads every thread's source issue once per connection, in one request.
    pub(super) fn sync_issue_links(&mut self, cx: &mut Context<Self>) {
        let epoch = self.host.connection.epoch;
        if cfg!(test)
            || !self.host.connection.connected
            || !matches!(self.linear.account.auth, Auth::SignedIn { .. })
            || self.issues.links_epoch == Some(epoch)
        {
            return;
        }
        self.issues.links_epoch = Some(epoch);
        self.linear_task(
            |client| {
                linear::list_thread_links(client, Some(500), None)
                    .map_err(|error| linear::classify(&error, false))
            },
            |this, result, _| {
                let Ok(page) = result else { return false };
                this.issues.links = page
                    .links
                    .into_iter()
                    .map(|link| (link.thread_id.clone(), link))
                    .collect();
                true
            },
            cx,
        );
    }

    /// Under a thread started from an issue: which one, and a way to it.
    pub(super) fn issue_link_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.host.thread.selected.as_ref()?;
        let link = self.issues.links.get(id)?;
        let url = link.url.clone();
        Some(
            row()
                .gap_2()
                .child(icon("issue"))
                .child(
                    // The label takes the free space and truncates within it; on its
                    // own in a row it could shrink to nothing.
                    tag(
                        "thread-issue-link",
                        format!("From {} · {}", link.identifier, link.title),
                        colors().subtle,
                    )
                    .flex_1()
                    .min_w_0()
                    .truncate(),
                )
                .child(
                    button("thread-issue-open", "Open in Linear")
                        .on_activate(cx, move |_, _, cx| cx.open_url(&url)),
                )
                .into_any_element(),
        )
    }
}
