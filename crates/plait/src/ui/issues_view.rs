//! The project's Linear issues, in a tab of their own. The host makes every
//! Linear request; this view asks for pages and details off the UI thread and
//! shows Linear's text as text. No link inside an issue is followed or fetched;
//! Open in Linear opens the issue's own https address in the browser.
use super::{Plait, settings, text_field::TextField, text_field::TextFieldEvent, theme::*};
use gpui::{prelude::*, *};
use plait_core::{
    host::linear::{self, IssueDetail, IssueSummary},
    linear_issues::{self, FILTERS, IssueRef, IssuesView, ListRequest, Load},
    linear_settings::Auth,
    tabs::{TabTarget, bookmark::BookmarkRecord},
};
use std::{collections::HashMap, path::PathBuf, time::Duration};

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
}

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
        self.open_tab_target(
            TabTarget::Issues {
                checkout: self.tab_checkout(project),
            },
            window,
            cx,
        );
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
        self.issues
            .view
            .target(self.host.connection.epoch, &checkout, restored);
        if cfg!(test)
            || !self.host.connection.connected
            || !matches!(self.linear.account.auth, Auth::SignedIn { .. })
        {
            return;
        }
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

    fn close_issue(&mut self, cx: &mut Context<Self>) {
        self.issues.view.clear_selection();
        self.save_issues_bookmark(cx);
        cx.notify();
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
        let url = issue.url.clone();
        let mut facts = vec![
            issue.state.name.clone(),
            linear_issues::priority(issue.priority).to_owned(),
        ];
        if let Some(assignee) = &issue.assignee {
            facts.push(assignee.name.clone());
        }
        facts.extend(issue.labels.iter().map(|label| label.name.clone()));
        let description = detail
            .description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("No description.");
        column()
            .gap_3()
            .child(
                row()
                    .gap_2()
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
                    .child(
                        toned_button("issue-start-thread", "Start thread", Tone::Primary)
                            .on_activate(cx, |this, window, cx| {
                                this.start_thread_from_issue(window, cx)
                            }),
                    )
                    .child(
                        button("issue-add-to-thread", "Add to thread…")
                            .on_activate(cx, |this, window, cx| {
                                this.choose_thread_for_issue(window, cx)
                            }),
                    )
                    .child(
                        button("issue-open-linear", "Open in Linear")
                            .on_activate(cx, move |_, _, cx| cx.open_url(&url)),
                    ),
            )
            .child(settings::heading(issue.title.clone()))
            .child(tag("issue-facts", facts.join(" · "), colors().muted))
            .child(plain_text("issue-description", description))
            .when(!detail.attachments.is_empty(), |el| {
                el.child(
                    tag(
                        "issue-attachments",
                        format!(
                            "Attachments, not opened by Plait: {}",
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
                el.child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Comments"),
                )
                .children(detail.comments.iter().enumerate().map(|(index, comment)| {
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
                        .child(plain_text(("issue-comment", index), &comment.body))
                }))
                .when(detail.comments_has_more, |el| {
                    el.child(tag(
                        "issue-comments-more",
                        "More comments are in Linear.",
                        colors().subtle,
                    ))
                })
            })
    }

    fn issue_detail_pane(&self, wide: bool, cx: &mut Context<Self>) -> Option<Div> {
        let view = &self.issues.view;
        view.selected.as_ref()?;
        let body = match (&view.detail, view.detail_load) {
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
        Some(
            column()
                .id("issue-detail")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .p_1()
                .child(body)
                .into_any_element()
                .into_div(),
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
        let header = row()
            .gap_2()
            .px_4()
            .py_2()
            .child(settings::heading("Linear issues"))
            .when_some(team, |el, team| {
                el.child(tag("issues-team", team, colors().subtle))
            })
            .child(div().flex_1())
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
                    .items_start()
                    .gap_4()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .child(
                        div()
                            .w(px(420.))
                            .flex_shrink_0()
                            .h_full()
                            .flex()
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

/// Linear's text as plain text: paragraphs kept, nothing interpreted or linked.
fn plain_text(id: impl Into<ElementId>, text: &str) -> Div {
    div()
        .id(id)
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(rgb(colors().text))
        .children(
            text.lines()
                .map(|line| div().min_h(px(20.)).child(line.to_owned()))
                .collect::<Vec<_>>(),
        )
        .into_div()
}

trait IntoDiv {
    fn into_div(self) -> Div;
}
impl<E: IntoElement> IntoDiv for E {
    fn into_div(self) -> Div {
        div().flex().flex_1().min_w_0().min_h_0().child(self)
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
            .threads
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
