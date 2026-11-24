use super::{
    Plait,
    selection::{Owner, Site},
    theme::*,
};
use gpui::{prelude::*, *};
use std::time::Instant;

pub(super) const LIST_TOP_PADDING: f32 = 8.;

/// Carries guarded keyboard actions to the key event that knows whether they repeat.
#[derive(Default)]
pub(super) struct ApprovalKeys {
    from_key: bool,
    action: Option<Box<dyn Action>>,
    /// The last key pressed without Command that has not come up yet.
    unreleased: Option<Keystroke>,
    /// Whether this key event presses `unreleased` again.
    repeat: bool,
    interceptor: Option<Subscription>,
}

impl ApprovalKeys {
    /// AppKit sends a key without Command through text input, which reports
    /// each repeat as a new press. Its key-up does arrive, so such a key
    /// repeats until it comes up. Command withholds key-up, and there the
    /// key-down's own repeat flag is reliable.
    fn pressed(&mut self, keystroke: &Keystroke) {
        self.from_key = true;
        self.action = None;
        self.repeat = self
            .unreleased
            .as_ref()
            .is_some_and(|held| held.key == keystroke.key && held.modifiers == keystroke.modifiers);
        self.unreleased = (!keystroke.modifiers.platform).then(|| keystroke.clone());
    }
}

/// Smooth reveal of the streaming reply and reasoning. Advanced once per
/// frame by `advance_reveals`; rows read how much to show.
#[derive(Default)]
pub(super) struct Reveals {
    text: Reveal,
    reasoning: Reveal,
    text_shown: usize,
    reasoning_shown: usize,
}

#[cfg(test)]
impl Reveals {
    pub fn pending_text(&self, total: usize) -> bool {
        self.text.pending(total)
    }

    /// Scene captures choose a prefix instead of depending on time between PNGs.
    pub fn set_scene_prefixes(&mut self, key: &str, text: usize, reasoning: usize) {
        let at = Instant::now() + std::time::Duration::from_secs(60);
        self.text = Reveal::default();
        self.reasoning = Reveal::default();
        // A production frame advances at least one byte even at zero elapsed time.
        self.text.advance(key, text.saturating_sub(1).max(1), at);
        self.reasoning
            .advance(key, reasoning.saturating_sub(1).max(1), at);
    }
}
use plait_client::{
    folding::{self, DisplayRow},
    history::{self, Key},
    host::{
        Approval, ApprovalMode, HistoryField, Message, MessageRole, Snapshot, ThreadStatus,
        harness::HarnessId,
    },
    markdown::Block,
    reveal::Reveal,
    transcript::{self, Row, Slot},
};

/// Full-width line above the conversation for host and submission state.
fn banner(tint: u32) -> Div {
    row()
        .w_full()
        .max_w(px(CONTENT_WIDTH + 48.))
        .mx_auto()
        .flex_shrink_0()
        .px_6()
        .py_2()
        .gap_3()
        .text_size(px(12.))
        .text_color(rgb(tint))
        .border_b_1()
        .border_color(rgb(colors().border))
}

/// The author and copy control share one line, clear of selectable text.
const AUTHOR_LINE: f32 = 22.;

fn message_author(author: &'static str) -> Div {
    div()
        .h(px(AUTHOR_LINE))
        .flex()
        .items_center()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(colors().muted))
        .child(author)
}

/// The turn line hangs in the middle of the conversation's 24-point left
/// margin, so text keeps the composer's edges and the marks stay clear of
/// the pane's edge in a narrow window.
const STRAND_LEFT: f32 = 12.;
/// The ring that marks a request, and the filled node that closes a turn or
/// shows its live state.
const RING: f32 = 9.;
const NODE: f32 = 7.;
/// Clear space around a mark, so the line does not touch it.
const MARK_GAP: f32 = 2.;
/// Height of a turn's record line and of the state line, which centers
/// their nodes.
const RECORD_LINE: f32 = 22.;

/// One row's piece of the turn line, from `from` points below the row's top
/// to its bottom. Rows are drawn edge to edge, so the pieces read as one line.
fn strand_line(from: f32) -> Div {
    div()
        .absolute()
        .left(px(-STRAND_LEFT))
        .top(px(from))
        .bottom_0()
        .w(px(1.))
        // The outline color, not the hairline: the line has to read as a
        // line beside dense text.
        .bg(rgb(colors().edge))
}

/// The line's last piece, down to the live state's node.
fn strand_end(to: f32) -> Div {
    div()
        .absolute()
        .left(px(-STRAND_LEFT))
        .top_0()
        .h(px(to))
        .w(px(1.))
        .bg(rgb(colors().edge))
}

/// A clear ground centered on the turn line, `center` points below its parent's top edge.
fn strand_mark_ground(center: f32, size: f32) -> Div {
    let ground = size + MARK_GAP * 2.;
    div()
        .absolute()
        .left(px(-STRAND_LEFT + 0.5 - ground / 2.))
        .top(px(center - ground / 2.))
        .size(px(ground))
        .rounded_full()
        .bg(rgb(colors().bg))
        .flex()
        .items_center()
        .justify_center()
}

fn strand_mark(center: f32, size: f32, color: u32, hollow: bool) -> Div {
    strand_mark_ground(center, size).child(
        div()
            .size(px(size))
            .rounded_full()
            .bg(rgb(color))
            .flex()
            .items_center()
            .justify_center()
            .when(hollow, |el| {
                el.child(
                    div()
                        .size(px(size - 3.))
                        .rounded_full()
                        .bg(rgb(colors().bg)),
                )
            }),
    )
}

/// A tool's path as Changes lists it: relative to the checkout.
fn checkout_relative(checkout: &std::path::Path, path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    path.strip_prefix(checkout).unwrap_or(path).to_path_buf()
}

/// Whether an approval shows enough to decide on. Pi's details are the call's
/// full arguments, so without them nothing is shown to approve. Another
/// harness names the call in `summary`; its details only add context, such as
/// a reason, and may be empty.
pub(super) fn approvable(approval: &Approval, harness: HarnessId) -> bool {
    approval.details.len() <= 16384 && (harness != HarnessId::Pi || !approval.details.is_empty())
}

const CONVERSATION_PADDING: f32 = 24.;

impl Plait {
    /// The desktop default from Settings → New threads.
    pub(super) fn default_approval(&self) -> ApprovalMode {
        if self.preferences.value.ask_before_actions {
            ApprovalMode::Ask
        } else {
            ApprovalMode::Auto
        }
    }

    /// The mode the next submission requests: the open thread's own choice,
    /// else the default. Version 1 hosts always ask. A harness thread asks for
    /// none: the harness's own mode governs it.
    /// A running thread reports its own mode; otherwise show what the next message uses.
    fn approval_note(&self, snapshot: Option<&Snapshot>) -> &'static str {
        let mode = match snapshot {
            Some(snapshot) if snapshot.status.busy() => snapshot.approval_mode,
            _ => self.next_approval_mode(),
        };
        if mode == Some(ApprovalMode::Auto) {
            "Automatic approval"
        } else {
            "Approve each action"
        }
    }

    /// An older host cannot change a thread's settings, so the box says what
    /// the thread runs with instead of offering controls: its project,
    /// machine and branch, its saved model, and how actions are approved.
    fn older_host_message_box_parts(&self) -> super::thread_controls::MessageBoxParts {
        let snapshot = self.host.thread.snapshot.as_ref();
        let model = snapshot.map(|snapshot| &snapshot.model).or_else(|| {
            self.host
                .connection
                .threads()
                .iter()
                .find(|thread| Some(&thread.id) == self.host.thread.selected.as_ref())
                .map(|thread| &thread.model)
        });
        let model_label = model.map_or_else(
            || "Loading saved model…".into(),
            |model| format!("{} / {}", model.provider, model.model_id),
        );
        super::thread_controls::MessageBoxParts {
            left: vec![
                self.connection_details(
                    Some(model_label),
                    Some((self.approval_note(snapshot.map(|s| &**s)), colors().muted)),
                )
                .into_any_element(),
            ],
            ..Default::default()
        }
    }

    pub(super) fn next_approval_mode(&self) -> Option<ApprovalMode> {
        if self.thread_harness() != HarnessId::Pi {
            return None;
        }
        let chosen = self
            .host
            .thread
            .selected
            .as_deref()
            .and_then(|id| self.thread_approval(id));
        (self.host.protocol() >= 2).then(|| chosen.unwrap_or_else(|| self.default_approval()))
    }

    fn live_message(&self, message: &Message, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let user = message.role == MessageRole::User;
        let site = Site {
            owner: Owner::Message(message.id.clone().into()),
            row: index,
            block: 0,
        };
        let body = (!message.text.is_empty() || message.images.is_empty()).then(|| {
            match self.workspace.formatted.messages.get(&message.id) {
                Some(parsed) if user => self.request(&site, &parsed.blocks),
                Some(parsed) => self.markdown(&site, &parsed.blocks, &message.id, cx),
                // The projection arrives with the snapshot; this is a one-frame fallback.
                None => prose().child(message.text.clone()).into_any_element(),
            }
        });
        let body = column()
            .w_full()
            .min_w_0()
            .gap_2()
            .children(self.history_parts(
                Key::Message(message.id.clone()),
                Some(HistoryField::Text),
                cx,
            ))
            .children(body)
            .when(!message.images.is_empty(), |el| {
                el.child(self.image_row(&message.images, cx))
            })
            .when(user, |el| el.p_3().rounded_md().bg(rgb(colors().panel)));
        let part = self
            .workspace
            .history
            .cache
            .get(&Key::Message(message.id.clone()))
            .is_some_and(|entry| {
                entry
                    .parts
                    .iter()
                    .any(|part| part.field == HistoryField::Text)
            });
        let copy = quiet_button(
            SharedString::from(format!("copy-message-{}", message.id)),
            if part {
                "Copy this part"
            } else {
                "Copy message"
            },
            row_group(index),
        )
        .aria_label(if part {
            "Copy this message part"
        } else {
            "Copy message source"
        })
        .on_activate(cx, {
            let snapshot = self
                .workspace
                .history
                .display
                .as_ref()
                .or(self.host.thread.snapshot.as_ref())
                .cloned();
            let id = message.id.clone();
            move |_, _, cx| {
                if let Some(message) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.messages.iter().find(|message| message.id == id))
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(message.text.clone()));
                }
            }
        });
        // The author stays visible even when the copy action is hidden.
        let id = (
            ElementId::from("live-message"),
            SharedString::from(message.id.clone()),
        );
        let label = self
            .workspace
            .formatted
            .message_labels
            .get(&message.id)
            .filter(|_| {
                self.workspace
                    .formatted
                    .messages
                    .get(&message.id)
                    .is_some_and(|parsed| parsed.source == message.text)
            })
            .map(SharedString::from)
            .unwrap_or_else(|| {
                format!(
                    "{} message: {}",
                    if user { "Your" } else { "Plait" },
                    message.text
                )
                .into()
            });
        column()
            .id(id)
            .role(Role::Group)
            .aria_label(label)
            .audited()
            .relative()
            .w_full()
            .min_w_0()
            .gap_1()
            .when(user, |el| {
                el.child(strand_mark(AUTHOR_LINE / 2., RING, colors().muted, true))
            })
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(message_author(if user { "You" } else { "Plait" }))
                    .child(copy.h(px(AUTHOR_LINE))),
            )
            .child(body)
            // A record stays on the open surface, never inside a request panel.
            .when_some(
                self.conversation_snapshot().and_then(|shown| {
                    history::row_record(
                        &self.workspace.formatted.turn_summaries,
                        shown,
                        &message.id,
                    )
                    .filter(|_| {
                        self.host.protocol() < 5
                            || !self
                                .workspace
                                .turn_records
                                .lines
                                .covered
                                .contains(&Key::Message(message.id.clone()))
                    })
                }),
                |el, summary| el.child(self.turn_record(&message.id, summary, cx)),
            )
            .into_any_element()
    }

    /// The record of the latest turn, once that turn has finished and its
    /// answer ends the rows. It is drawn with the turn's state where the
    /// line ends, not in the list.
    fn latest_record(&self) -> Option<(&str, &history::TurnSummary)> {
        let live = self.host.thread.snapshot.as_deref()?;
        let last = self.conversation_snapshot()?.messages.last()?;
        // While older history is being read the rows stop short of the
        // latest turn, and the state below is not that of their last row.
        if live.status.busy()
            || live
                .messages
                .last()
                .is_none_or(|latest| latest.id != last.id)
        {
            return None;
        }
        let summary = self.workspace.formatted.turn_summaries.get(&last.id)?;
        Some((&last.id, summary))
    }

    /// What a finished turn did, in the words and controls of one line: its
    /// steps and the files its edits changed, each opening in Changes.
    fn record_parts(
        &self,
        id: &str,
        summary: &history::TurnSummary,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        /// Files named before the rest are counted.
        const NAMED: usize = 3;
        let word = |text: String, color: u32| {
            div()
                .h(px(RECORD_LINE))
                .flex()
                .items_center()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        // A turn that only answered has nothing to count: its node says it closed.
        if summary.steps == 0 {
            return Vec::new();
        }
        let checkout = self
            .conversation_snapshot()
            .map(|snapshot| snapshot.checkout.clone())
            .unwrap_or_default();
        let mut parts = vec![word(summary.steps_label(), colors().muted)];
        if summary.files.is_empty() {
            return parts;
        }
        parts.push(word("·".into(), colors().subtle));
        parts.push(word("edited".into(), colors().subtle));
        parts.extend(summary.files.iter().take(NAMED).map(|path| {
            let target = checkout_relative(&checkout, path);
            let name = target
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
            button(
                (
                    ElementId::from("turn-file"),
                    SharedString::from(format!("{id}:{path}")),
                ),
                "",
            )
            .h(px(RECORD_LINE))
            .px_1()
            .aria_label(format!("Show {} in Changes", target.display()))
            // A name opens the file in Changes, so it is drawn as links are:
            // blue, and underlined so it does not depend on the color.
            .child(
                div()
                    .font_family(mono())
                    .text_size(px(code_size()))
                    .text_color(rgb(colors().blue))
                    .underline()
                    .text_decoration_color(rgb(colors().blue))
                    .child(name),
            )
            .on_activate(cx, move |this, window, cx| {
                this.open_thread_file(target.clone(), window, cx)
            })
            .into_any_element()
        }));
        let more = summary.files.len().saturating_sub(NAMED);
        if more > 0 {
            parts.push(word(format!("and {more} more"), colors().subtle));
        }
        parts
    }

    /// Where an earlier turn closes on the turn line: a node and its record.
    /// A turn that only answered closes at a node alone, in less space.
    fn turn_record(
        &self,
        id: &str,
        summary: &history::TurnSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        /// Height of the row that holds a node and no words.
        const BARE: f32 = 12.;
        let parts = self.record_parts(id, summary, cx);
        let line = if parts.is_empty() { BARE } else { RECORD_LINE };
        row()
            .id(SharedString::from(format!("turn-record-{id}")))
            .role(Role::Group)
            .aria_label(format!("This turn: {}", summary.label()))
            .audited()
            .relative()
            .overflow_x_hidden()
            .gap_x(px(6.))
            .tooltip({
                let tip = self.legacy_record_tip();
                move |_, cx| {
                    cx.new(|_| super::thread_controls::TextTooltip(tip.clone()))
                        .into()
                }
            })
            .pt(px(2.))
            .min_h(px(2. + line))
            .text_size(px(12.))
            .child(strand_mark(2. + line / 2., NODE, colors().subtle, false))
            .children(parts)
            .into_any_element()
    }

    /// The answer being written, with a still mark that it is not finished.
    /// Move the streaming reveals one frame on, and ask for the next frame
    /// while either trails what has arrived. Once per frame, from `render`.
    pub(super) fn advance_reveals(&mut self, window: &mut Window) {
        let key = self.host.thread.selected.as_deref().unwrap_or_default();
        let now = Instant::now();
        let total = |parsed: Option<&std::sync::Arc<plait_client::markdown::Message>>| {
            parsed.map_or(0, |parsed| plait_client::reveal::total(&parsed.blocks))
        };
        let text = total(self.workspace.formatted.live.as_ref());
        let reasoning = total(self.workspace.formatted.live_reasoning.as_ref());
        let reveal = &mut self.workspace.reveal;
        reveal.text_shown = reveal.text.advance(key, text, now);
        reveal.reasoning_shown = reveal.reasoning.advance(key, reasoning, now);
        if reveal.text.pending(text) || reveal.reasoning.pending(reasoning) {
            window.request_animation_frame();
        }
    }

    /// The part of live `blocks` on screen this frame: whole blocks and a cut last one.
    fn revealed<'a>(&self, shown: usize, blocks: &'a [Block]) -> (&'a [Block], Option<Block>) {
        let (whole, tail) = plait_client::reveal::visible(blocks, shown);
        (&blocks[..whole], tail)
    }

    fn live_stream(&self, text: &str, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let site = Site {
            owner: Owner::Stream,
            row: index,
            block: 0,
        };
        let body = match self.workspace.formatted.live.as_ref() {
            Some(parsed) => {
                let (whole, tail) = self.revealed(self.workspace.reveal.text_shown, &parsed.blocks);
                self.markdown_with(&site, whole, tail.as_ref(), "stream", cx)
            }
            None => prose().child(text.to_owned()).into_any_element(),
        };
        // The live state is named once, where the turn line ends.
        column()
            .id("live-stream")
            .role(Role::Group)
            .aria_label(
                self.workspace
                    .formatted
                    .live_label
                    .as_ref()
                    .filter(|_| {
                        self.workspace
                            .formatted
                            .live
                            .as_ref()
                            .is_some_and(|parsed| parsed.source == text)
                    })
                    .map(SharedString::from)
                    .unwrap_or_else(|| format!("Plait is writing: {text}").into()),
            )
            .audited()
            .w_full()
            .min_w_0()
            .gap_1()
            .child(message_author("Plait"))
            .child(body)
            .into_any_element()
    }

    fn reasoning(&self, id: &str, index: usize, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let open = live || self.workspace.history.expanded.contains(id);
        let site = Site {
            owner: if live {
                Owner::LiveReasoning
            } else {
                Owner::Reasoning(id.to_owned().into())
            },
            row: index,
            block: 0,
        };
        let parsed = if live {
            self.workspace.formatted.live_reasoning.as_ref()
        } else {
            self.workspace.formatted.reasoning.get(id)
        };
        let key = id.to_owned();
        // The chevron says open or closed, as on a tool call; the words are
        // for assistive technology. The state line names live thinking.
        let caption = if live {
            "Reasoning, in progress"
        } else if open {
            "Hide reasoning"
        } else {
            "Show reasoning"
        };
        let heading = row()
            .justify_between()
            .gap_2()
            .child(
                button(SharedString::from(format!("reasoning-{id}")), "")
                    .px_1()
                    .ml(px(-TOOL_INSET))
                    .aria_label(caption)
                    .aria_expanded(open)
                    .child(icon(if open { "down" } else { "right" }))
                    .child(div().text_size(px(13.)).child("Reasoning"))
                    .disabled(live)
                    .when(!live, |el| {
                        el.on_activate(cx, move |this, _, cx| this.toggle_reasoning(&key, cx))
                    }),
            )
            .when(open, |el| {
                el.child(
                    quiet_button(
                        SharedString::from(format!("copy-reasoning-{id}")),
                        "Copy reasoning",
                        row_group(index),
                    )
                    .on_activate(cx, {
                        let snapshot = self
                            .workspace
                            .history
                            .display
                            .as_ref()
                            .or(self.host.thread.snapshot.as_ref())
                            .cloned();
                        let id = id.to_owned();
                        move |_, _, cx| {
                            if let Some(snapshot) = &snapshot {
                                let text = if live {
                                    Some(&snapshot.live_reasoning)
                                } else {
                                    snapshot
                                        .messages
                                        .iter()
                                        .find(|message| message.id == id)
                                        .map(|message| &message.reasoning)
                                };
                                if let Some(text) = text {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                }
                            }
                        }
                    }),
                )
            });
        column()
            .gap_1()
            .text_color(rgb(colors().muted))
            .child(heading)
            .when(open, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("reasoning-body-{id}")))
                        .pl_3()
                        .border_l_1()
                        .border_color(rgb(colors().border))
                        .when(live, |el| el.max_h(px(180.)).overflow_y_scroll())
                        .when_some(parsed, |el, parsed| {
                            let (whole, tail) = if live {
                                self.revealed(self.workspace.reveal.reasoning_shown, &parsed.blocks)
                            } else {
                                (&parsed.blocks[..], None)
                            };
                            el.child(self.markdown_with(
                                &site,
                                whole,
                                tail.as_ref(),
                                &format!("reasoning-{id}"),
                                cx,
                            ))
                        })
                        .when(!live, |el| {
                            el.children(self.history_parts(
                                Key::Message(id.to_owned()),
                                Some(HistoryField::Reasoning),
                                cx,
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    /// One folded earlier turn on the line: its ring with the first line of the
    /// request and age at the right, then its node with the closing line unchanged.
    /// Does not build any folded-away reply, reasoning, or tool content.
    fn folded_turn_row(
        &self,
        index: usize,
        snapshot: &Snapshot,
        turn_id: &str,
        request_message: usize,
        closing_slot: Option<Slot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const TOP: f32 = 16.;
        const BOTTOM: f32 = 8.;
        let Some(request) = snapshot.messages.get(request_message) else {
            return div().into_any_element();
        };
        let closing_row = closing_slot.and_then(|slot| transcript::resolve(snapshot, slot));
        let recorded = closing_row
            .and_then(history::records::closing_key)
            .and_then(|key| self.workspace.turn_records.lines.rows.get(&key))
            .filter(|_| self.host.protocol() >= 5);
        let legacy = closing_row.and_then(|row| match row {
            Row::Message(message) => history::row_record(
                &self.workspace.formatted.turn_summaries,
                snapshot,
                &message.id,
            )
            .filter(|_| {
                self.host.protocol() < 5
                    || !self
                        .workspace
                        .turn_records
                        .lines
                        .covered
                        .contains(&Key::Message(message.id.clone()))
            })
            .map(|summary| (message.id.as_str(), summary)),
            _ => None,
        });
        let preview = folding::first_request_line(&request.text, request.images.len());
        let age = recorded
            .filter(|line| line.created_at > 0)
            .map(|line| {
                let now = if self.workspace.history.now > 0 {
                    self.workspace.history.now
                } else {
                    super::commits::now().max(0) as u64 * 1000
                };
                plait_client::navigation::age(Some(line.created_at), now)
            })
            .filter(|age| !age.is_empty());
        let accessible_name = match (&preview[..], &age) {
            ("", Some(age)) => format!("Turn hidden, {age}. Show turn"),
            ("", None) => "Turn hidden. Show turn".into(),
            (text, Some(age)) => format!("Turn hidden: {text}, {age}. Show turn"),
            (text, None) => format!("Turn hidden: {text}. Show turn"),
        };
        let open_id = turn_id.to_owned();
        let disclosure = div()
            .relative()
            .w_full()
            .min_w_0()
            .child(strand_mark(ROW_HEIGHT / 2., RING, colors().muted, true))
            .child(
                button(
                    (
                        ElementId::from("folded-turn"),
                        SharedString::from(turn_id.to_owned()),
                    ),
                    "",
                )
                .w_full()
                .min_w_0()
                .h(px(ROW_HEIGHT))
                .px_2()
                .ml(px(-8.))
                .justify_between()
                .gap_2()
                .when(cfg!(test), |el| {
                    let mut context = KeyContext::default();
                    context.set("folded-turn", turn_id.to_owned());
                    el.key_context(context)
                })
                .aria_label(accessible_name)
                .aria_expanded(false)
                .child(
                    row()
                        .min_w_0()
                        .flex_1()
                        .gap(px(6.))
                        .child(icon("right"))
                        .child(div().min_w_0().truncate().text_size(px(13.)).child(preview)),
                )
                .when_some(age, |el, age| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(rgb(colors().subtle))
                            .child(age),
                    )
                })
                .on_activate(cx, move |this, _, cx| this.open_folded_turn(&open_id, cx)),
            );
        let from = if index == 0 {
            TOP + ROW_HEIGHT / 2.
        } else {
            0.
        };
        div()
            .group(row_group(index))
            .w_full()
            .px_6()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .mx_auto()
                    .pt(px(TOP))
                    .pb(px(BOTTOM))
                    .child(strand_line(from))
                    .child(disclosure)
                    .when_some(recorded, |el, line| {
                        el.child(
                            div()
                                .relative()
                                .pt_1()
                                .child(strand_mark(
                                    4. + RECORD_LINE / 2.,
                                    NODE,
                                    colors().subtle,
                                    false,
                                ))
                                .child(self.record_line(line, false, row_group(index), cx)),
                        )
                    })
                    .when(recorded.is_none(), |el| {
                        el.when_some(legacy, |el, (id, summary)| {
                            el.child(self.turn_record(id, summary, cx))
                        })
                    }),
            )
            .into_any_element()
    }

    fn live_event(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(snapshot) = self.conversation_snapshot() else {
            return div().into_any_element();
        };
        let slot = match self.workspace.live_layout.rows.get(index) {
            Some(DisplayRow::Slot(slot)) => *slot,
            Some(DisplayRow::FoldedTurn {
                id,
                request_message,
                closing_slot,
            }) => {
                return self.folded_turn_row(
                    index,
                    snapshot,
                    id,
                    *request_message,
                    *closing_slot,
                    cx,
                );
            }
            None => return div().into_any_element(),
        };
        let Some(row) = transcript::resolve(snapshot, slot) else {
            return div().into_any_element();
        };
        // Requests stand clear of the previous turn. Replies and streaming
        // text use the same spacing, so completion does not move the author.
        let request = matches!(row, Row::Message(message) if message.role == MessageRole::User);
        let recorded = history::records::closing_key(row)
            .and_then(|key| self.workspace.turn_records.lines.rows.get(&key))
            .filter(|_| self.host.protocol() >= 5);
        let (content, top, bottom) = match row {
            Row::Message(message) if request => (self.live_message(message, index, cx), 24., 8.),
            Row::Message(message) => (self.live_message(message, index, cx), 16., 8.),
            Row::Streaming(text) => (self.live_stream(text, index, cx), 16., 8.),
            Row::Reasoning(message) => (self.reasoning(&message.id, index, false, cx), 6., 3.),
            Row::LiveReasoning(_) => (self.reasoning("live", index, true, cx), 6., 3.),
            Row::Tool(tool) => match self.tool_row(tool, index, cx) {
                Some(content) => (content, 1., 1.),
                // Folded into a closed group: the row stays, with no height.
                None if recorded.is_some() => (div().into_any_element(), 1., 1.),
                None => return div().into_any_element(),
            },
        };
        // The line starts with the first row: at the ring of a request, and
        // otherwise where the row's content begins, not in the space above it.
        let from = match (index, request) {
            (0, true) => top + AUTHOR_LINE / 2.,
            (0, false) => top,
            _ => 0.,
        };
        // The padding is inside the column, so the line spans the whole row.
        div()
            .group(row_group(index))
            .w_full()
            .px(px(CONVERSATION_PADDING))
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .mx_auto()
                    .pt(px(top))
                    .pb(px(bottom))
                    .child(strand_line(from))
                    .child(content)
                    .when_some(recorded, |el, line| {
                        el.child(
                            div()
                                .relative()
                                .pt_1()
                                .child(strand_mark(
                                    4. + RECORD_LINE / 2.,
                                    NODE,
                                    colors().subtle,
                                    false,
                                ))
                                .child(self.record_line(line, false, row_group(index), cx)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The list of rows with what floats over it: pointer handling for text
    /// selection, the way back to the newest row, and a hovered link's address.
    /// Whether the list is scrolled away from its newest row.
    fn reading_back(&self) -> bool {
        self.workspace.live_scroll.logical_scroll_top().item_ix < self.workspace.live_count
    }

    fn live_rows(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        self.reconcile_tool_rows();
        self.resolve_selection();
        let away = self.reading_back();
        let size = (
            snapshot.messages.len() + snapshot.tools.len(),
            snapshot.live_text.len() + snapshot.live_reasoning.len(),
        );
        if !away {
            self.workspace.reading.seen.set(size);
        }
        let unseen = self.workspace.reading.seen.get() != size;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(self.selection_surface(cx))
            .child(
                list(
                    self.workspace.live_scroll.clone(),
                    cx.processor(|this, index, _, cx| this.live_event(index, cx)),
                )
                .size_full()
                // No space below: the turn line runs on into the state line.
                .pt(px(LIST_TOP_PADDING)),
            )
            .when(away, |el| {
                el.child(
                    // Laid out like a row, so the control ends at the text's right
                    // edge and stays clear of a hovered link's address on the left.
                    div()
                        .absolute()
                        .bottom(px(10.))
                        .left_0()
                        .right_0()
                        .px(px(CONVERSATION_PADDING))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .w_full()
                                .max_w(px(CONTENT_WIDTH))
                                .mx_auto()
                                .child(
                                    toned_button("jump-to-latest", "", Tone::Filled)
                                        .occlude()
                                        .gap(px(6.))
                                        .aria_label(if unseen {
                                            "New activity. Jump to latest"
                                        } else {
                                            "Jump to latest"
                                        })
                                        .when(unseen, |el| el.child(dot(colors().blue)))
                                        .child(if unseen {
                                            "New activity · Jump to latest"
                                        } else {
                                            "Jump to latest"
                                        })
                                        .child(icon("down").text_color(rgb(colors().text)))
                                        .on_activate(cx, |this, window, cx| {
                                            this.jump_latest(window, cx)
                                        }),
                                ),
                        ),
                )
            })
            .when_some(self.workspace.reading.hovered_link.clone(), |el, url| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(6.))
                        .left(px(8.))
                        .max_w(relative(0.6))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(colors().panel))
                        .border_1()
                        .border_color(rgb(colors().edge))
                        .text_size(px(11.))
                        .text_color(rgb(colors().muted))
                        .truncate()
                        .child(a11y_text("link-destination", format!("Opens {url}"))),
                )
            })
            .into_any_element()
    }

    /// Whether `approval` can be answered now. Deny once, Allow once, and
    /// their shortcuts all pass through this one gate.
    pub(super) fn can_answer(&self, approval: &Approval) -> bool {
        approvable(approval, self.thread_harness())
            && self.host.connection.connected
            && !self.host.thread.controlling
            && !self.host.thread.decided_approvals.contains(&approval.id)
    }

    /// What a Deny Once or Allow Once shortcut would send: the open thread's
    /// pending approval, while its card is on screen and its buttons would act.
    pub(super) fn shortcut_answer(&self, allow: bool) -> Option<(String, bool)> {
        // The card is drawn only in the open thread's conversation, and a
        // palette or picker opens over it, so an answer is never given to
        // arguments the person cannot see. A command chosen in the palette
        // runs after the palette has closed.
        let shown = !self.settings_open
            && self.workspace.tab == super::Workspace::Conversation
            && self.overlay.is_none();
        let snapshot = self.host.thread.snapshot.as_ref().filter(|snapshot| {
            shown && self.host.thread.selected.as_deref() == Some(snapshot.id.as_str())
        })?;
        let approval = snapshot.pending_approval.as_ref()?;
        self.can_answer(approval)
            .then(|| (approval.id.clone(), allow))
    }

    /// Deny Once or Allow Once from the keyboard, the menu, or the palette.
    fn answer_by_shortcut(&mut self, allow: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(answer) = self.shortcut_answer(allow) {
            self.control_thread(Some(answer), window, cx);
        }
    }

    /// GPUI resolves actions before delivering KeyDown, which carries is_held.
    pub(super) fn defer_key_action(&mut self, action: &dyn Action, cx: &mut Context<Self>) -> bool {
        if !self.approval_keys.from_key {
            return false;
        }
        // Propagation may visit another binding; retain the highest-priority action.
        self.approval_keys
            .action
            .get_or_insert_with(|| action.boxed_clone());
        cx.propagate();
        true
    }

    pub(super) fn watch_approval_keys(&mut self, window: &Window, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        let window_id = window.window_handle().window_id();
        self.approval_keys.interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let _ = entity.update(cx, |this, _| this.approval_keys.pressed(&event.keystroke));
            // Clear before the accepted action is redispatched; menu actions need no key event.
            let entity = entity.clone();
            cx.defer(move |cx| {
                let _ = entity.update(cx, |this, _| {
                    this.approval_keys.from_key = false;
                    this.approval_keys.action = None;
                });
            });
        }));
    }

    pub(super) fn on_approval_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, action: &super::DenyOnce, window, cx| {
            if !this.defer_key_action(action, cx) {
                this.answer_by_shortcut(false, window, cx);
            }
        }))
        .on_action(cx.listener(|this, action: &super::AllowOnce, window, cx| {
            if !this.defer_key_action(action, cx) {
                this.answer_by_shortcut(true, window, cx);
            }
        }))
        .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if let Some(action) = this.approval_keys.action.take() {
                cx.stop_propagation();
                if !event.is_held && !this.approval_keys.repeat {
                    window.dispatch_action(action, cx);
                }
            }
        }))
        // Capture, so no control below can keep a release from rearming.
        .capture_key_up(cx.listener(|this, _: &KeyUpEvent, _, _| {
            this.approval_keys.unreleased = None;
        }))
        .on_modifiers_changed(cx.listener(|this, _: &ModifiersChangedEvent, _, _| {
            this.approval_keys.unreleased = None;
        }))
    }

    /// A control's shortcut as the keymap binds it now. Nothing when unbound.
    fn shortcut_hint(&self, id: &str) -> Option<Div> {
        let label = self.shortcut_label(id);
        (!label.is_empty())
            .then(|| key_hint(SharedString::from(format!("shortcut-hint-{id}")), label))
    }

    /// Exact arguments and a one-call decision. Nothing runs without it.
    fn approval(&self, approval: &Approval, cx: &mut Context<Self>) -> impl IntoElement {
        let exact = approvable(approval, self.thread_harness());
        let enabled = self.can_answer(approval);
        let allow_id = approval.id.clone();
        let deny_id = approval.id.clone();
        let dirty = approval.dirty_paths.join(", ");
        // Pi refuses to write files that had changes before the run; another
        // harness only lists them, and nothing stops it writing them.
        let dirty_note = match self.thread_harness() {
            _ if dirty.is_empty() => String::new(),
            HarnessId::Pi => format!("Protected dirty paths: {dirty}"),
            id => format!(
                "Changed before this run: {dirty}. {} is not stopped from writing them.",
                self.harness_row(id)
                    .map_or_else(|| id.label().into(), |row| row.name.clone())
            ),
        };
        div()
            .flex_shrink_0()
            .px(px(CONVERSATION_PADDING))
            .pt_2()
            .child(
                column()
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .mx_auto()
                    .p_3()
                    .gap_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(colors().warn_edge))
                    .bg(rgb(colors().rail))
                    .text_size(px(12.))
                    .child(
                        row().child(dot(colors().amber)).child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("Approval required: {}", approval.summary)),
                        ),
                    )
                    .when(!approval.details.is_empty(), |el| {
                        el.child(
                            div()
                                .id("approval-details")
                                .role(Role::Group)
                                .aria_label(format!(
                                    "Exact tool arguments: {}. Risk: {}. {}",
                                    approval.details, approval.risk, dirty_note
                                ))
                                .audited()
                                .max_h(px(140.))
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(rgb(colors().bg))
                                .border_1()
                                .border_color(rgb(colors().border))
                                .font_family(mono())
                                .text_size(px(code_size()))
                                .line_height(px(code_line_height()))
                                .child(approval.details.clone()),
                        )
                    })
                    // The dot, title, and edge already ask for attention; the
                    // risk is read, so it is set as secondary text.
                    .child(
                        div()
                            .text_color(rgb(colors().muted))
                            .child(approval.risk.clone()),
                    )
                    .when(!approval.dirty_paths.is_empty(), |el| {
                        el.child(
                            div()
                                .text_color(rgb(colors().muted))
                                .child(dirty_note.clone()),
                        )
                    })
                    .when(!exact, |el| {
                        el.child(div().text_color(rgb(colors().red)).child(
                            "Exact arguments are unavailable. Approval is disabled; Stop this run.",
                        ))
                    })
                    .child(
                        row()
                            .justify_end()
                            .child(
                                toned_button("deny-tool", "Deny once", Tone::Filled)
                                    .aria_keyshortcuts(self.shortcut_aria("approval.deny_once"))
                                    .children(self.shortcut_hint("approval.deny_once"))
                                    .disabled(!enabled)
                                    .when(enabled, |el| {
                                        el.on_activate(cx, move |this, window, cx| {
                                            this.control_thread(
                                                Some((deny_id.clone(), false)),
                                                window,
                                                cx,
                                            )
                                        })
                                    }),
                            )
                            .child(
                                toned_button("allow-tool", "Allow once", Tone::Primary)
                                    .aria_keyshortcuts(self.shortcut_aria("approval.allow_once"))
                                    .children(self.shortcut_hint("approval.allow_once"))
                                    .disabled(!enabled)
                                    .when(enabled, |el| {
                                        el.on_activate(cx, move |this, window, cx| {
                                            this.control_thread(
                                                Some((allow_id.clone(), true)),
                                                window,
                                                cx,
                                            )
                                        })
                                    }),
                            ),
                    ),
            )
    }

    fn live_banners(&self, pending: bool, cx: &mut Context<Self>) -> Vec<Div> {
        let mut banners = Vec::new();
        if let Some(error) = self.host.connection.error.clone() {
            let recovery = self.has_identity_recovery(&error);
            banners.push(
                banner(colors().red)
                    .child(a11y_text(
                        "host-connection-error",
                        self.identity_recovery_message(&error),
                    ))
                    .when(recovery, |el| el.child(self.identity_recovery_button(cx))),
            );
        }
        if !self.host.connection.connected {
            let connecting = self.host.connection.connecting;
            banners.push(
                banner(colors().amber)
                    .child(div().flex_1().min_w_0().child(
                        "Disconnected · displayed content may be stale. The host may still be running this thread.",
                    ))
                    .child(
                        toned_button(
                            "reconnect-conversation",
                            if connecting { "Connecting…" } else { "Reconnect" },
Tone::Filled,
)
                        .disabled(connecting)
                        .when(!connecting, |el| {
                            el.on_activate(cx, |this, window, cx| this.reconnect_host(window, cx))
                        }),
                    ),
            );
        }
        if pending && self.host.protocol() < 3 {
            let ready = self.host.connection.connected
                && !self.host.thread.submitting
                && self.session.ready();
            banners.push(
                banner(colors().muted)
                    .child(div().flex_1().min_w_0().child(if self.host.thread.submitting {
                        "Waiting for host acceptance…"
                    } else {
                        "Submission unconfirmed. Retry uses the same request ID and cannot start duplicate work."
                    }))
                    .child(
                        toned_button("retry-submit", "Retry safely", Tone::Filled)
                            .disabled(!ready)
                            .when(ready, |el| {
                                el.on_activate(cx, |this, window, cx| {
                                    this.retry_submission(window, cx)
                                })
                            }),
                    ),
            );
        }
        banners
    }

    pub(super) fn live_conversation(
        &self,
        _wide: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.host.thread.snapshot.clone();
        let pending = self
            .host
            .thread
            .selected
            .as_ref()
            .is_some_and(|id| self.host.submissions.pending(id).is_some());
        let mut main = column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .children(self.conversation_find_bar(cx))
            .children(self.live_banners(pending, cx))
            .child(self.history_heading(cx))
            .child(if snapshot.is_none() {
                empty(
                    "Loading thread",
                    "The conversation comes from the host. Reconnect if the host is unavailable.",
                )
                .into_any_element()
            } else if self.workspace.live_count == 0 {
                empty(
                    "Start this conversation",
                    "Send a message below. Change the model or thinking level between runs.",
                )
                .into_any_element()
            } else if let Some(snapshot) = &snapshot {
                self.live_rows(snapshot, cx)
            } else {
                div().into_any_element()
            });
        if let Some(snapshot) = &snapshot {
            let state = self
                .preparation_label()
                .map(|label| (label.to_owned(), false))
                .or_else(|| history::run_state(snapshot, self.workspace.history.now));
            // The turn line runs from the rows to the state's node. It is
            // broken while the rows on screen stop short of the latest turn:
            // scrolled back from the newest row, or reading older history.
            let joined = self.workspace.live_count > 0
                && state.is_some()
                && !self.workspace.history.cache.detached
                && !self.reading_back();
            if snapshot.truncated && !self.workspace.history.enabled {
                main = main.child(
                    div().px(px(CONVERSATION_PADDING)).child(
                        div()
                            .relative()
                            .w_full()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .py_1()
                            .when(joined, |el| el.child(strand_line(0.)))
                            .child(tag(
                                "conversation-truncated",
                                "Bounded preview · older messages or tool output have been clipped by the host.",
                                colors().amber,
                            )),
                    ),
                );
            }
            if let Some(line) = self
                .workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .filter(|_| self.host.protocol() >= 5 && !snapshot.status.busy())
            {
                main = main.child(
                    div().px(px(CONVERSATION_PADDING)).child(
                        div()
                            .relative()
                            .w_full()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .pt_2()
                            .pb(px(6.))
                            .when(joined, |el| el.child(strand_end(8. + RECORD_LINE / 2.)))
                            .child(strand_mark(
                                8. + RECORD_LINE / 2.,
                                NODE,
                                colors().muted,
                                false,
                            ))
                            .child(self.record_line(line, true, "latest-run".into(), cx)),
                    ),
                );
            } else if let Some((state, failed)) = state {
                /// Space above the state's line.
                const LEAD: f32 = 8.;
                // The node takes the thread's state color, as the rail does;
                // a finished turn is quiet.
                let tint = match snapshot.status {
                    ThreadStatus::Idle => colors().muted,
                    status => host_status_color(status),
                };
                // The turn line ends here, at the state of its latest turn
                // and, once that turn has finished, its record.
                let center = LEAD + RECORD_LINE / 2.;
                let record = self.latest_record().map(|(id, summary)| {
                    (
                        format!("{state} · {}", summary.label()),
                        self.record_parts(id, summary, cx),
                    )
                });
                let (name, parts) = record.unwrap_or_else(|| (state.clone(), Vec::new()));
                main = main.child(
                    div().px(px(CONVERSATION_PADDING)).child(
                        row()
                            .id("turn-state")
                            .role(Role::Group)
                            .aria_label(name)
                            .audited()
                            .tooltip({
                                let tip = self.legacy_record_tip();
                                move |_, cx| {
                                    cx.new(|_| super::thread_controls::TextTooltip(tip.clone()))
                                        .into()
                                }
                            })
                            .relative()
                            // The node stays beside the first line of a state
                            // that wraps, such as a long failure.
                            .items_start()
                            .flex_wrap()
                            .gap_x(px(6.))
                            .w_full()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .pt(px(LEAD))
                            .pb(px(6.))
                            .text_size(px(12.))
                            .when(joined, |el| el.child(strand_end(center)))
                            .child(
                                if self.host.connection.connected
                                    && snapshot.status == ThreadStatus::Running
                                {
                                    strand_mark_ground(center, 13.).child(running_mark(px(13.)))
                                } else {
                                    strand_mark(center, NODE, tint, false)
                                },
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .line_height(px(RECORD_LINE))
                                    .text_color(rgb(if failed {
                                        colors().red
                                    } else {
                                        colors().muted
                                    }))
                                    .child(state),
                            )
                            .when(!parts.is_empty(), |el| {
                                el.child(
                                    div()
                                        .h(px(RECORD_LINE))
                                        .flex()
                                        .items_center()
                                        .text_color(rgb(colors().subtle))
                                        .child("·"),
                                )
                                .children(parts)
                            }),
                    ),
                );
            }
            if self.workspace.history.enabled
                && snapshot.status.busy()
                && self.host.thread.live_preview_clipped
            {
                main = main.child(
                    div().px(px(CONVERSATION_PADDING)).child(
                        div()
                            .w_full()
                            .max_w(px(CONTENT_WIDTH))
                            .mx_auto()
                            .text_size(px(11.))
                            .text_color(rgb(colors().amber))
                            .child(
                                "Live preview · complete text loads when this response is saved.",
                            ),
                    ),
                );
            }
            if let Some(approval) = &snapshot.pending_approval {
                main = main.child(self.approval(approval, cx));
            }
        }
        let composer = self
            .host
            .thread
            .selected
            .as_ref()
            .and_then(|id| self.workspace.composers.get(id));
        main = main.child(
            column()
                .flex_shrink_0()
                .px(px(CONVERSATION_PADDING))
                .pt_2()
                .pb_3()
                .gap_2()
                .children(self.checkout_wait_control(cx))
                .children(self.queued_inputs(cx))
                .children(self.thread_files_card(cx))
                .when_some(composer.cloned(), |el, composer| {
                    let parts = if self.host.protocol() >= 3 {
                        self.thread_message_box_parts(cx)
                    } else {
                        self.older_host_message_box_parts()
                    };
                    el.child(self.message_box(composer, parts, window, cx))
                })
                // The source issue shows whatever the host's protocol.
                .children(self.issue_link_line(cx).map(|line| {
                    div()
                        .w_full()
                        .max_w(px(CONTENT_WIDTH))
                        .mx_auto()
                        .child(line)
                })),
        );
        row()
            .gap_0()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .when(!self.panel_fills(), |el| el.child(main))
            .when(self.panel_is_open(), |el| el.child(self.review_panel(cx)))
    }
}

/// Hover group for one conversation row, so its quiet controls appear together.
fn row_group(row: usize) -> SharedString {
    format!("conversation-row-{row}").into()
}

#[cfg(test)]
mod tests {
    use super::{Approval, HarnessId, approvable};

    #[test]
    fn only_pi_needs_the_full_arguments_to_approve() {
        let approval = |details: &str| Approval {
            details: details.into(),
            id: "a1".into(),
            tool_id: "t1".into(),
            kind: plait_client::host::ApprovalKind::Command,
            summary: "git status".into(),
            dirty_paths: Vec::new(),
            risk: String::new(),
        };
        assert!(approvable(
            &approval("{\"command\":\"git\"}"),
            HarnessId::Pi
        ));
        assert!(!approvable(&approval(""), HarnessId::Pi));
        // Codex and Claude Code name the call in the summary.
        assert!(approvable(&approval(""), HarnessId::Claude));
        assert!(approvable(&approval(""), HarnessId::Codex));
        let long = "x".repeat(16385);
        assert!(!approvable(&approval(&long), HarnessId::Claude));
    }
}
