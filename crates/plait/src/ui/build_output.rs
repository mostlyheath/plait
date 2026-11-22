//! Prepared policy and pushed progress; render only reads this state.
use super::{
    Plait, settings,
    text_field::{TextField, TextFieldEvent},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_core::host::{
    self, CancelWatch,
    build_output::{self, Info, Kind, Phase, Progress, Request},
};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Default)]
pub(super) struct State {
    pub key: Option<(u64, PathBuf)>,
    pub info: Option<Info>,
    pub busy: bool,
    pub note: Option<String>,
    pub field: Option<Entity<TextField>>,
    changed: Option<Subscription>,
    pub progress: BTreeMap<String, (Kind, Progress)>,
    watches: BTreeMap<String, CancelWatch>,
    generations: BTreeMap<String, u64>,
    generation: u64,
    label: Option<(String, Kind, String)>,
}
impl State {
    pub(super) fn prepare_progress(&mut self) {
        self.label = self
            .progress
            .iter()
            .find(|(_, (_, p))| p.phase == Phase::Cloning)
            .map(|(id, (kind, p))| {
                (
                    id.clone(),
                    *kind,
                    format!("Cloning build output · {} files", p.files),
                )
            });
    }
    fn receive(&mut self, request: &str, generation: u64, kind: Kind, progress: Progress) -> bool {
        if self.generations.get(request) != Some(&generation) {
            return false;
        }
        self.progress.insert(request.into(), (kind, progress));
        self.prepare_progress();
        true
    }
}
impl Plait {
    pub(super) fn prepare_build_settings(&mut self, cx: &mut Context<Self>) {
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        if self.build_output.key.as_ref() == Some(&key) || self.build_output.busy {
            return;
        }
        self.build_output.key = Some(key);
        self.build_output.info = None;
        self.build_output.note = None;
        let field = cx.new(|cx| TextField::new("Build directory", "Relative directory", cx));
        self.build_output.changed =
            Some(cx.subscribe(&field, |_, _, _: &TextFieldEvent, cx| cx.notify()));
        self.build_output.field = Some(field);
        if !cfg!(test) {
            self.request_build_setting(None, false, cx);
        }
    }
    /// `refresh` is the Refresh control's: the host refreshes the seed now.
    fn request_build_setting(
        &mut self,
        paths: Option<Vec<String>>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        if self.build_output.busy || !self.host.connection.connected {
            return;
        }
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        let expected = self.settings_checkout_key(&key.1).cloned();
        let previous = self.build_output.info.clone();
        let root = key.1.clone();
        self.build_output.busy = true;
        let client = self.host_client();
        let supported = self.host.connection.has_checkout_identity;
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let identity =
                Self::identify_settings_checkout(&client, &root, supported, local, expected)?;

            let info = if let Some(paths) = paths {
                let info = previous.ok_or_else(|| {
                    host::Error::Protocol("Load build output choices before saving.".into())
                })?;
                Some(build_output::set(
                    &client,
                    &Request::SetBuildOutput {
                        checkout: identity.root.clone(),
                        identity: info.policy.identity,
                        revision: info.policy.revision,
                        paths,
                    },
                )?)
            } else {
                build_output::info(&client, identity.root.clone(), refresh)?
            };
            Self::identify_settings_checkout(
                &client,
                &identity.root,
                supported,
                local,
                Some(identity.clone()),
            )?;
            Ok::<_, host::Error>((info, identity))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.build_output.key.as_ref() != Some(&key) {
                    return;
                }
                this.build_output.busy = false;
                match result.and_then(|(info, identity)| {
                    this.remember_settings_checkout_key(&key, identity)?;
                    Ok(info)
                }) {
                    Ok(info) => {
                        this.build_output.note = info
                            .is_none()
                            .then(|| "Build output choices need a newer host.".into());
                        this.build_output.info = info;
                    }
                    Err(error) => this.build_output.note = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn approve_build_directory(&mut self, value: String, cx: &mut Context<Self>) {
        let Some(info) = &self.build_output.info else {
            return;
        };
        let mut paths = info.policy.paths.clone();
        if paths.contains(&value) {
            return;
        }
        if let Err(error) = plait_core::build_output::relative(&value) {
            self.build_output.note = Some(error);
            cx.notify();
            return;
        }
        paths.push(value);
        self.request_build_setting(Some(paths), false, cx);
    }
    pub(super) fn build_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.build_output.info.is_some()
            && !self.build_output.busy
            && self.host.connection.connected;
        let mut content = column().gap_2().min_w_0();
        if let Some(info) = &self.build_output.info {
            if info.policy.paths.is_empty() {
                content = content.child(tag("build-empty", "None approved", colors().muted));
            }
            if let Some(seed) = &info.seed {
                // A labelled node, not a plain tag, so the size reaches assistive technology.
                let label = format!("Seed · {}", super::worktrees_view::bytes(seed.bytes));
                content = content.child(div().id("build-seed").role(Role::Label).aria_label(label.clone())
                    .text_size(px(11.)).text_color(rgb(colors().muted)).child(StyledText::new(label))
                    .tooltip(|_, cx| cx.new(|_| TextTooltip("Logical size of the build seed new worktrees are cloned from. Its blocks are shared with the checkout until either changes, so this is not space removing it would free; once the checkout is rebuilt, the seed keeps the old blocks. It is removed when no directory is approved.".into())).into()).audited());
            }
            for (i, path) in info.policy.paths.iter().enumerate() {
                let path = path.clone();
                let value = path.clone();
                content = content.child(row().gap_2().flex_wrap()
                    .child(tag(("build-path", i), path, colors().text))
                    .child(button(("build-remove", i), "Remove").disabled(!enabled)
                        .tooltip(|_, cx| cx.new(|_| TextTooltip("Stop treating this directory as build output. Existing output stays; it will block worktree removal until approved again or removed manually.".into())).into())
                        .on_activate(cx, move |this, _, cx| {
                            if let Some(info) = &this.build_output.info { let paths = info.policy.paths.iter().filter(|p| **p != value).cloned().collect(); this.request_build_setting(Some(paths), false, cx); }
                        })));
            }
            for (i, path) in info
                .suggestions
                .iter()
                .filter(|p| !info.policy.paths.contains(p))
                .enumerate()
            {
                let value = path.clone();
                content = content.child(row().gap_2().flex_wrap()
                    .child(tag(("build-suggestion", i), format!("Suggested · {path}"), colors().muted))
                    .child(button(("build-approve", i), "Approve").disabled(!enabled)
                        .tooltip(|_, cx| cx.new(|_| TextTooltip("Approve this ignored directory for copy-on-write cloning and removal with its worktree. A manifest only suggests it; nothing is approved automatically.".into())).into())
                        .on_activate(cx, move |this, _, cx| this.approve_build_directory(value.clone(), cx))));
            }
        } else {
            content = content.child(tag("build-unavailable", "Unavailable", colors().muted));
        }
        if let Some(field) = &self.build_output.field {
            let field = field.clone();
            let valid = enabled && !field.read(cx).text().trim().is_empty();
            content = content.child(row().gap_2().child(div().flex_1().min_w_0().child(field.clone()))
                .child(button("build-custom-approve", "Approve").disabled(!valid)
                    .tooltip(|_, cx| cx.new(|_| TextTooltip("Approve one relative, ignored directory of reproducible build output. Its contents go with a removed worktree. Python environments are refused; caches with absolute paths should be recreated.".into())).into())
                    .on_activate(cx, move |this, _, cx| this.approve_build_directory(field.read(cx).text().trim().to_owned(), cx))));
        }
        content = content.when(self.build_output.busy, |el| el.child(tag("build-loading", "Checking", colors().muted)))
            .when_some(self.build_output.note.clone(), |el, note| el.child(tag("build-note", note, colors().amber)))
            .child(button("build-refresh", "Refresh").disabled(self.build_output.busy || !self.host.connection.connected)
                .tooltip(|_, cx| cx.new(|_| TextTooltip("Read the host's approved list, look for suggestions in project manifests, and refresh the build seed now; an unchanged source is not cloned again.".into())).into())
                .on_activate(cx, |this, _, cx| this.request_build_setting(None, true, cx)));
        settings::setting("Build output", "Approved directories can give new worktrees a warm start. Shared blocks are not reclaimable space.", content).into_any_element()
    }
    pub(super) fn finish_build_watch(&mut self, request: &str, generation: u64) {
        if self.build_output.generations.get(request) != Some(&generation) {
            return;
        }
        self.build_output.watches.remove(request);
        self.build_output.generations.remove(request);
        if self
            .build_output
            .progress
            .get(request)
            .is_some_and(|(_, p)| !p.done())
        {
            self.build_output.progress.remove(request);
        }
        self.build_output.prepare_progress();
    }
    pub(super) fn watch_build_output(
        &mut self,
        request: String,
        kind: Kind,
        cx: &mut Context<Self>,
    ) -> u64 {
        if let Some(generation) = self.build_output.generations.get(&request) {
            return *generation;
        }
        self.build_output.generation = self.build_output.generation.wrapping_add(1);
        let generation = self.build_output.generation;
        self.build_output
            .generations
            .insert(request.clone(), generation);
        self.build_output.progress.retain(|_, (_, p)| !p.done());
        self.build_output.progress.insert(
            request.clone(),
            (
                kind,
                Progress {
                    schema: 1,
                    worktree_build_output_v1: true,
                    request_id: request.clone(),
                    thread_id: None,
                    phase: Phase::Waiting,
                    files: 0,
                    outcome: None,
                },
            ),
        );
        self.build_output.prepare_progress();
        let epoch = self.host.connection.epoch;
        let id = request.clone();
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { build_output::subscribe(&client, id, kind) });
        cx.spawn(async move |this, cx| {
            let mut updates = match task.await {
                Ok(updates) => updates,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.build_output.generations.get(&request) == Some(&generation) {
                            this.finish_build_watch(&request, generation);
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let cancel = updates.cancel_handle();
            if this
                .update(cx, |this, _| {
                    if this.host.connection.epoch != epoch
                        || this.build_output.generations.get(&request) != Some(&generation)
                    {
                        return false;
                    }
                    this.build_output.watches.insert(request.clone(), cancel);
                    true
                })
                .ok()
                != Some(true)
            {
                return;
            }
            while let Some(Ok(progress)) = updates.next().await {
                let done = progress.done();
                if this
                    .update(cx, |this, cx| {
                        if this.host.connection.epoch != epoch {
                            return false;
                        }
                        if !this
                            .build_output
                            .receive(&request, generation, kind, progress)
                        {
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                    || done
                {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.build_output.generations.get(&request) == Some(&generation) {
                    this.finish_build_watch(&request, generation);
                    cx.notify();
                }
            });
        })
        .detach();
        generation
    }
    pub(super) fn build_progress_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, kind, label) = self.build_output.label.as_ref()?;
        let (id, kind, label) = (id.clone(), *kind, label.clone());
        let client = self.host_client();
        Some(row().px_4().py_2().gap_2().flex_wrap()
            .child(tag("build-progress", label, colors().muted))
            .child(button("build-skip", "Skip").tooltip(|_, cx| cx.new(|_| TextTooltip("Discard this clone and start the worktree cold. The first message waits until partial output has been removed.".into())).into())
                .on_activate(cx, move |_, _, cx| {
                    let request = id.clone();
                    let client = client.clone();
        let task = cx.background_executor().spawn(async move { build_output::skip(&client, request, kind) });
                    cx.spawn(async move |this, cx| { if let Err(error) = task.await { let _ = this.update(cx, |this, cx| { this.notice = Some(error.to_string()); cx.notify(); }); } }).detach();
                })).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[core::prelude::v1::test]
    fn retired_progress_cannot_replace_the_current_creation() {
        let mut state = State::default();
        state.generations.insert("request".into(), 2);
        let progress = Progress {
            schema: 1,
            worktree_build_output_v1: true,
            request_id: "request".into(),
            thread_id: None,
            phase: Phase::Cloning,
            files: 42,
            outcome: None,
        };
        assert!(!state.receive("request", 1, Kind::Worktree, progress.clone()));
        assert!(state.label.is_none());
        assert!(state.receive("request", 2, Kind::Worktree, progress));
        assert_eq!(
            state.label.as_ref().unwrap().2,
            "Cloning build output · 42 files"
        );
    }
}
