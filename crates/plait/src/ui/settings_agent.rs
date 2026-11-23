//! A project's settings → Your Pi agent: what the person's Pi setup offers
//! threads in the open checkout, read from the host with the same discovery a
//! run makes.
//!
//! Turning a skill or template off stays in Pi's own settings,
//! which Plait follows, so there is one place to manage them. Every string the
//! page and the composer draw is made once, on the background executor, when
//! the reply arrives; rendering formats nothing.

use super::{Plait, composer::AgentCommand, settings, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::host::{
    agent::{AgentResources, McpServer, McpTransport, Request},
    providers,
};
use std::{path::PathBuf, sync::Arc};

/// Rows each list shows before Show all.
const FOLDED: usize = 12;
/// Characters of a description drawn in one line, here and in the composer.
const DETAIL: usize = 140;

#[derive(Default)]
pub(super) struct AgentState {
    pub(super) key: Option<(u64, PathBuf)>,
    pub(super) view: Option<Arc<AgentView>>,
    pub(super) busy: bool,
    pub(super) revision: u64,
    note: Option<String>,
    expanded: bool,
    /// Given to composers before the setup is read, so an unchanged empty list costs nothing.
    empty: Arc<[AgentCommand]>,
}

/// One line of a list: what it is, what it says, where it came from, and its file.
pub(super) struct Row {
    /// The row's element id: its list, title and path, so it stays the same
    /// when rows are added or removed around it. Set by [`with_keys`].
    key: SharedString,
    title: SharedString,
    detail: SharedString,
    tag: SharedString,
    path: SharedString,
    folder: Option<PathBuf>,
}

pub(super) struct AgentView {
    trusted: bool,
    instructions: Option<Row>,
    skills: Vec<Row>,
    templates: Vec<Row>,
    problems: Vec<Row>,
    pub(super) mcp_servers: Vec<McpRow>,
    pub(super) commands: Arc<[AgentCommand]>,
}

pub(super) struct McpRow {
    pub(super) name: SharedString,
    pub(super) state: SharedString,
    transport: &'static str,
    pub(super) hover: SharedString,
    label: SharedString,
    pub(super) enabled: bool,
    can_enable: bool,
    pub(super) file: Option<PathBuf>,
    definition: Option<String>,
}

impl From<McpServer> for McpRow {
    fn from(server: McpServer) -> Self {
        use plait_client::host::agent::{McpConsent, McpState};
        let state = if server.consent == Some(McpConsent::Changed) {
            "Changed"
        } else if server.consent == Some(McpConsent::Required) {
            "Needs review"
        } else if server.consent == Some(McpConsent::Restricted) && server.enabled {
            "User limits"
        } else if server.state == Some(McpState::Failed) && server.needs_sign_in == Some(true) {
            "Sign in with Pi"
        } else if server.state == Some(McpState::Failed)
            || (server.state.is_some() && server.problem.is_some())
        {
            "Problem"
        } else if server.problem.is_some() {
            "Invalid"
        } else if server.needs_sign_in == Some(true) && !server.signed_in {
            "Sign in with Pi"
        } else if server.signed_in {
            "Sign-in saved"
        } else if !server.enabled {
            "Off"
        } else if server
            .disabled_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("Provider authentication"))
        {
            "Pi auth planned"
        } else if server.state == Some(McpState::Connected) {
            "Connected"
        } else if server.state == Some(McpState::Stopped) {
            "Stopped"
        } else {
            "Ready to search"
        };
        let state = match server.tool_count {
            Some(count) if server.state.is_some() => format!("{state} · {count} tools"),
            _ => state.into(),
        };
        let mut hover = vec![server.source.to_string_lossy().into_owned()];
        if let Some(source) = &server.override_source {
            hover.push(format!("Project override: {}", source.display()));
        }
        if let Some(program) = &server.program {
            hover.push(format!("Starts: {program}"));
            if let Some(count) = server.argument_count {
                hover.push(format!("{count} arguments; their values are hidden here. Read the defining file before turning this server on."));
            }
        } else if let Some(host) = &server.host {
            hover.push(format!("Connects to: {host}"));
        }
        hover.push("Environment and header values and credential commands are always hidden. Once on, local servers can read the project and use the network from startup, including when sandboxed and before any call is approved. Approval gates calls, not the server's own behaviour.".into());
        if let Some(reason) = server.disabled_reason.as_ref().or(server.problem.as_ref()) {
            hover.push(reason.clone());
        }
        if server.needs_sign_in == Some(true) && !server.signed_in {
            hover.push("Sign in with Pi. Using Pi's browser sign-in is planned.".into());
        } else if server.signed_in {
            hover.push("Pi holds a saved sign-in; its validity has not been checked. Using it in Plait is planned.".into());
        } else if server.needs_sign_in.is_none() && server.problem.is_none() {
            hover.push("Whether this server needs sign-in is unknown until it connects.".into());
        }
        if server.tool_count.is_some() {
            hover.push(
                "Tool count observed during the latest run that connected this server.".into(),
            );
        }
        Self {
            label: format!(
                "{}: {} for this project",
                server.name,
                if server.enabled { "On" } else { "Off" }
            )
            .into(),
            name: server.name.into(),
            state: state.into(),
            transport: match server.transport {
                Some(McpTransport::Stdio) => "stdio",
                Some(McpTransport::Http) => "HTTP",
                _ => "Unknown",
            },
            hover: plait_client::display_text::escape(&hover.join("\n")).into(),
            enabled: server.enabled,
            can_enable: server.can_enable,
            file: None,
            definition: server.definition,
        }
    }
}

/// The first line, cut to `DETAIL` characters.
fn line(text: &str) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    match first.char_indices().nth(DETAIL) {
        Some((cut, _)) => format!("{}…", &first[..cut]),
        None => first.to_owned(),
    }
}

/// Give each row a key from its list, title and path, numbering repeats so
/// two identical rows still differ. Built once, not on every frame.
fn with_keys(list: &str, rows: impl Iterator<Item = Row>) -> Vec<Row> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    rows.map(|mut row| {
        let base = format!("{list}\n{}\n{}", row.title, row.path);
        let repeat = seen.entry(base.clone()).or_default();
        row.key = if *repeat == 0 {
            base.into()
        } else {
            format!("{base}\n{repeat}").into()
        };
        *repeat += 1;
        row
    })
    .collect()
}

fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.)
    }
}

impl AgentView {
    pub(super) fn new(reply: AgentResources) -> Self {
        let path = |path: &std::path::Path| SharedString::from(home_relative(path));
        let folder = |path: &std::path::Path| path.parent().map(std::path::Path::to_path_buf);
        let skills = reply.skills.iter().map(|skill| Row {
            key: SharedString::default(),
            title: format!("/skill:{}", skill.name).into(),
            detail: line(&skill.description).into(),
            tag: if skill.model_invocation {
                skill.origin.label().into()
            } else {
                format!("{} · only when you type it", skill.origin.label()).into()
            },
            path: path(&skill.path),
            folder: folder(&skill.path),
        });
        let templates = reply.templates.iter().map(|template| Row {
            key: SharedString::default(),
            title: match &template.argument_hint {
                Some(hint) => format!("/{} {hint}", template.name).into(),
                None => format!("/{}", template.name).into(),
            },
            detail: line(&template.description).into(),
            tag: template.origin.label().into(),
            path: path(&template.path),
            folder: folder(&template.path),
        });
        let problems = reply.problems.iter().map(|problem| Row {
            key: SharedString::default(),
            title: problem.message.clone().into(),
            detail: SharedString::default(),
            tag: SharedString::default(),
            path: problem.path.as_deref().map(path).unwrap_or_default(),
            folder: problem.path.as_deref().and_then(folder),
        });
        // A name with a space can never be typed as one command, so it is not offered.
        let commands = reply
            .skills
            .iter()
            .map(|skill| (format!("skill:{}", skill.name), line(&skill.description)))
            .chain(reply.templates.iter().map(|template| {
                let detail = match &template.argument_hint {
                    Some(hint) => line(&format!("{hint} · {}", template.description)),
                    None => line(&template.description),
                };
                (template.name.clone(), detail)
            }))
            .filter(|(name, _)| !name.contains(char::is_whitespace))
            .map(|(name, detail)| AgentCommand::new(name, detail))
            .collect();
        Self {
            trusted: reply.trusted,
            mcp_servers: reply
                .mcp_servers
                .into_iter()
                .map(|server| {
                    let file = server
                        .override_source
                        .as_ref()
                        .unwrap_or(&server.source)
                        .strip_prefix(&reply.checkout)
                        .ok()
                        .filter(|path| {
                            !path.as_os_str().is_empty()
                                && path
                                    .components()
                                    .all(|part| matches!(part, std::path::Component::Normal(_)))
                        })
                        .map(PathBuf::from);
                    let mut row = McpRow::from(server);
                    row.file = file;
                    row
                })
                .collect(),
            instructions: reply.instructions.as_ref().map(|file| Row {
                key: "agent-instructions".into(),
                title: "User instructions".into(),
                detail: size(file.bytes).into(),
                tag: SharedString::default(),
                path: path(&file.path),
                folder: folder(&file.path),
            }),
            skills: with_keys("agent-skills", skills),
            templates: with_keys("agent-templates", templates),
            problems: with_keys("agent-problems", problems),
            commands,
        }
    }
}

impl Plait {
    /// The open checkout's skills and templates for the composer; none until read.
    pub(super) fn agent_commands(&self) -> Arc<[AgentCommand]> {
        match &self.agent.view {
            Some(view)
                if self.agent.key.as_ref()
                    == Some(&(
                        self.host.connection.epoch,
                        self.host.creation.checkout.clone(),
                    )) =>
            {
                view.commands.clone()
            }
            _ => self.agent.empty.clone(),
        }
    }

    /// Read the setup once per host connection and checkout. Opening the
    /// project's settings, or Refresh there, reads it again.
    pub(super) fn sync_agent(&mut self, cx: &mut Context<Self>) {
        if !self.host.connection.connected
            || self.state.repository.snapshot().ready().is_none()
            || cfg!(test)
        {
            return;
        }
        let key = (
            self.host.connection.epoch,
            self.host.creation.checkout.clone(),
        );
        if self.agent.key.as_ref() == Some(&key) {
            return;
        }
        self.agent = AgentState {
            key: Some(key),
            revision: self.agent.revision.wrapping_add(1),
            expanded: self.agent.expanded,
            empty: self.agent.empty.clone(),
            ..Default::default()
        };
        self.refresh_agent(cx);
    }

    /// Read the setup again: Pi's files can change while the app is open.
    pub(super) fn refresh_agent(&mut self, cx: &mut Context<Self>) {
        self.request_agent(self.host.creation.checkout.clone(), None, cx);
    }

    pub(super) fn refresh_project_agent(&mut self, cx: &mut Context<Self>) {
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        let state = self.project_agent_mut();
        if state.key.as_ref() != Some(&key) {
            *state = AgentState {
                key: Some(key.clone()),
                revision: state.revision.wrapping_add(1),
                ..Default::default()
            };
        }
        self.request_agent(key.1, None, cx);
    }

    fn request_project_agent(
        &mut self,
        change: Option<(String, bool, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.request_agent(self.settings_checkout().to_owned(), change, cx);
    }

    fn request_agent(
        &mut self,
        checkout: PathBuf,
        change: Option<(String, bool, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        if !self.host.connection.connected || cfg!(test) {
            return;
        }
        let key = (self.host.connection.epoch, checkout.clone());
        let Some(state) = self.agent_for_mut(&checkout) else {
            return;
        };
        if state.busy {
            return;
        }
        state.busy = true;
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state.note = None;
        let checkout = if checkout == self.host.creation.checkout {
            self.state
                .repository
                .snapshot()
                .ready()
                .map_or(checkout, |snapshot| snapshot.root.clone())
        } else {
            checkout
        };
        let checkout_generation =
            (key.1 == self.host.creation.checkout).then_some(self.host.creation.generation);
        let expected = self.settings_checkout_key(&key.1).cloned();
        let client = self.host_client();
        let supported = self.host.connection.has_checkout_identity;
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let identity =
                Self::identify_settings_checkout(&client, &checkout, supported, local, expected)?;
            let canonical = identity.root.clone();
            let request = match change {
                Some((name, enabled, definition)) => {
                    Request::set_mcp_server(canonical.clone(), name, enabled, definition)
                }
                None => Request::AgentResources {
                    checkout: canonical.clone(),
                    identity: supported.then(|| plait_client::host::git::WireIdentity {
                        dev: identity.device.to_string(),
                        ino: identity.inode.to_string(),
                    }),
                    protocol_version: 3,
                },
            };
            let reply: AgentResources = client.send(&request)?;
            if reply.checkout != canonical {
                return Err(plait_client::host::Error::Protocol(
                    "Host returned another checkout's agent setup.".into(),
                ));
            }
            Self::identify_settings_checkout(
                &client,
                &identity.root,
                supported,
                local,
                Some(identity.clone()),
            )?;
            Ok((AgentView::new(reply), identity))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if checkout_generation
                    .is_some_and(|generation| this.host.creation.generation != generation)
                {
                    return;
                }
                let result = result.and_then(|(reply, identity)| {
                    this.remember_settings_checkout_key(&key, identity)?;
                    Ok(reply)
                });
                if !this.accept_agent(&key, revision, result) {
                    return;
                }
                this.sync_composer(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// A refresh or saved switch must still belong to the same connection and checkout.
    pub(super) fn accept_agent(
        &mut self,
        key: &(u64, PathBuf),
        revision: u64,
        result: Result<AgentView, plait_client::host::Error>,
    ) -> bool {
        if self.host.connection.epoch != key.0 {
            return false;
        }
        let Some(state) = self.agent_for_mut(&key.1) else {
            return false;
        };
        if state.revision != revision || state.key.as_ref() != Some(key) {
            return false;
        }
        state.busy = false;
        match result {
            Ok(view) => state.view = Some(Arc::new(view)),
            Err(error) => {
                state.note = Some(if providers::needs_host_update(&error) {
                    "Update the host to manage your Pi setup.".into()
                } else {
                    providers::refusal(&error)
                })
            }
        }
        true
    }

    /// Trust changes which project skills and templates load, so read them again.
    pub(super) fn forget_agent(&mut self) {
        self.agent.key = None;
        self.agent.revision = self.agent.revision.wrapping_add(1);
    }

    pub(super) fn agent_settings(&self, cx: &mut Context<Self>) -> Div {
        let refresh = toned_button(
            "agent-refresh",
            if self.project_agent().busy {
                "Reading…"
            } else {
                "Refresh"
            },
            Tone::Filled,
        )
        .disabled(self.project_agent().busy || !self.host.connection.connected)
        .when(
            !self.project_agent().busy && self.host.connection.connected,
            |el| el.on_activate(cx, |this, _, cx| this.refresh_project_agent(cx)),
        );
        let mut page = settings::section("Your Pi agent").child(settings::setting(
            "What threads here are offered",
            "Skills, prompt templates, instructions, and MCP servers from your Pi setup. Skills and templates follow Pi's settings.",
            refresh,
        ));
        if let Some(note) = &self.project_agent().note {
            page = page
                .child(tag("agent-setup-note", note.clone(), colors().amber).line_height(px(16.)));
        }
        let Some(view) = self.project_agent().view.as_ref() else {
            let page = page.when(self.project_agent().note.is_none(), |el| {
                el.child(tag(
                    "agent-setup-status",
                    if self.host.connection.connected {
                        "Reading your agent setup…"
                    } else {
                        "Connect the host to read your agent setup."
                    },
                    colors().subtle,
                ))
            });
            return page;
        };
        if !view.trusted {
            page = page.child(tag(
                "agent-setup-untrusted",
                "Skills and templates inside this project load only when its commands are trusted, above.",
                colors().subtle,
            ).line_height(px(16.)));
        }
        let instructions = match &view.instructions {
            Some(row) => self.rows(std::slice::from_ref(row), true, cx),
            None => column().child(tag(
                "agent-instructions-empty",
                "No AGENTS.md or CLAUDE.md in the Pi agent folder.",
                colors().subtle,
            )),
        };
        column()
            .gap_4()
            .child(page)
            .when(!view.mcp_servers.is_empty(), |el| {
                el.child(self.mcp_settings(&view.mcp_servers, cx))
            })
            .child(settings::section("Instructions").child(instructions))
            .child(self.list(
                "Skills",
                "agent-skills",
                &view.skills,
                "No skills found.",
                cx,
            ))
            .child(self.list(
                "Prompt templates",
                "agent-templates",
                &view.templates,
                "No prompt templates found.",
                cx,
            ))
            .when(!view.problems.is_empty(), |el| {
                el.child(self.list("Problems", "agent-problems", &view.problems, "", cx))
            })
    }

    fn mcp_settings(&self, servers: &[McpRow], cx: &mut Context<Self>) -> Div {
        let mut section = settings::section("MCP servers").child(tag(
            "mcp-intro",
            "Pi finds tools on demand. Each server stops when the run ends.",
            colors().subtle,
        ));
        let all = self.project_agent().expanded || servers.len() <= FOLDED;
        for (index, server) in servers
            .iter()
            .take(if all { servers.len() } else { FOLDED })
            .enumerate()
        {
            let hover = server.hover.clone();
            let name = server.name.clone();
            let definition = server.definition.clone();
            let enabled = !server.enabled;
            let available = server.can_enable
                && self.host.connection.connected
                && !self.project_agent().busy
                && self.project_agent().key.as_ref()
                    == Some(&(
                        self.host.connection.epoch,
                        self.settings_checkout().to_owned(),
                    ));
            section = section.child(
                row()
                    .id(("mcp-server", index))
                    .min_w_0()
                    .gap_2()
                    .h(px(28.))
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.to_string())).into())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(server.name.clone()),
                    )
                    .child(tag("mcp-transport", server.transport, colors().subtle).flex_shrink_0())
                    .child(tag("mcp-state", server.state.clone(), colors().muted).flex_shrink_0())
                    .when_some(server.file.clone(), |el, path| {
                        el.child(button(("mcp-open-file", index), "Open file").on_activate(
                            cx,
                            move |this, window, cx| {
                                let checkout = this.settings_checkout().to_owned();
                                this.show_settings(false, window, cx);
                                if checkout != this.host.creation.checkout {
                                    this.change_checkout(checkout, window, cx);
                                }
                                this.open_file(path.clone(), None, cx);
                            },
                        ))
                    })
                    .child(
                        chip(
                            ("mcp-switch", index),
                            if server.enabled { "On" } else { "Off" },
                            server.enabled,
                        )
                        .role(Role::Switch)
                        .aria_label(server.label.clone())
                        .disabled(!available)
                        .when(available, |el| {
                            el.on_activate(cx, move |this, _, cx| {
                                this.request_project_agent(
                                    Some((name.to_string(), enabled, definition.clone())),
                                    cx,
                                );
                            })
                        }),
                    ),
            );
        }
        section.when(!all, |el| {
            el.child(
                button("mcp-show-all", format!("Show all {}", servers.len())).on_activate(
                    cx,
                    |this, _, cx| {
                        this.project_agent_mut().expanded = true;
                        cx.notify();
                    },
                ),
            )
        })
    }

    fn list(
        &self,
        title: &'static str,
        id: &'static str,
        rows: &[Row],
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let all = self.project_agent().expanded || rows.len() <= FOLDED;
        let shown = if all { rows } else { &rows[..FOLDED] };
        settings::section(title)
            .when(rows.is_empty(), |el| {
                el.child(tag(
                    (ElementId::from("agent-empty"), SharedString::from(id)),
                    empty,
                    colors().subtle,
                ))
            })
            .child(self.rows(shown, false, cx))
            .when(!all, |el| {
                el.child(div().child(
                    button((id, usize::MAX), format!("Show all {}", rows.len())).on_activate(
                        cx,
                        |this, _, cx| {
                            this.project_agent_mut().expanded = true;
                            cx.notify();
                        },
                    ),
                ))
            })
    }

    fn rows(&self, rows: &[Row], quiet: bool, cx: &mut Context<Self>) -> Div {
        let mut list = column().gap_3();
        for item in rows {
            let open = item.folder.clone();
            let mut text = column().flex_1().min_w_0().gap(px(2.)).child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(if quiet { colors().muted } else { colors().text }))
                    .child(a11y_text("agent-row-title", item.title.clone())),
            );
            if !item.detail.is_empty() {
                text = text
                    .child(tag("agent-row-detail", item.detail.clone(), colors().muted).truncate());
            }
            let mut place = row().gap_2().min_w_0();
            if !item.tag.is_empty() {
                place = place
                    .child(tag("agent-row-tag", item.tag.clone(), colors().subtle).flex_shrink_0());
            }
            if !item.path.is_empty() {
                place = place
                    .child(tag("agent-row-path", item.path.clone(), colors().subtle).truncate());
            }
            // Keyed by the row, not its index, so ids hold as rows come and go.
            list = list.child(
                row()
                    .id(ElementId::Name(item.key.clone()))
                    .items_start()
                    .gap_3()
                    .child(text.child(place))
                    .when_some(open, |el, folder| {
                        el.child(
                            toned_button("agent-row-open", "Open folder", Tone::Ghost)
                                .flex_shrink_0()
                                .on_activate(cx, move |_, _, cx| cx.open_with_system(&folder)),
                        )
                    }),
            );
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(message: &str, path: &str) -> Row {
        Row {
            key: SharedString::default(),
            title: message.to_owned().into(),
            detail: SharedString::default(),
            tag: SharedString::default(),
            path: path.to_owned().into(),
            folder: None,
        }
    }

    #[core::prelude::v1::test]
    fn row_keys_are_unique_and_do_not_depend_on_position() {
        let first = with_keys(
            "agent-problems",
            [
                problem("bad", "a.md"),
                problem("bad", "a.md"),
                problem("bad", "b.md"),
            ]
            .into_iter(),
        );
        let keys: Vec<_> = first.iter().map(|row| row.key.clone()).collect();
        assert_eq!(keys.len(), 3);
        assert!(keys[0] != keys[1] && keys[1] != keys[2] && keys[0] != keys[2]);

        // A row added in front leaves the others' keys alone.
        let second = with_keys(
            "agent-problems",
            [
                problem("new", "c.md"),
                problem("bad", "a.md"),
                problem("bad", "b.md"),
            ]
            .into_iter(),
        );
        assert_eq!(second[1].key, keys[0]);
        assert_eq!(second[2].key, keys[2]);
    }
}
