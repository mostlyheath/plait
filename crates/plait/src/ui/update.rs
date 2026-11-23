use super::*;
use crate::update::install;
use plait_client::update::{self, Offer};
use std::sync::Arc;

/// Release update progress. Network and install work run on the background executor.
#[derive(Clone, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Current,
    Available(Arc<Offer>),
    Installing(Arc<Offer>),
    Failed(String),
}

impl Plait {
    /// `quiet` checks run at launch: failures stay out of the way until asked.
    pub(super) fn check_for_updates(&mut self, quiet: bool, cx: &mut Context<Self>) {
        if matches!(
            self.updates,
            UpdateState::Checking | UpdateState::Installing(_)
        ) {
            return;
        }
        self.updates = UpdateState::Checking;
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { update::check(update::current_version().as_ref()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.updates = match result {
                    Ok(Some(offer)) => UpdateState::Available(Arc::new(offer)),
                    Ok(None) => UpdateState::Current,
                    Err(_) if quiet => UpdateState::Idle,
                    Err(error) => UpdateState::Failed(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let UpdateState::Available(offer) = self.updates.clone() else {
            return;
        };
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            self.updates = UpdateState::Failed("No home directory to install into.".into());
            cx.notify();
            return;
        };
        self.updates = UpdateState::Installing(offer.clone());
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            let bundle = install::apply_app_update(&offer, &home)?;
            install::relaunch(&bundle)?;
            Ok::<_, update::Error>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                // Quit the normal way so drafts and layout are saved; the
                // relaunch waits for this process to exit.
                Ok(()) => this.close_desktop(true, window, cx),
                Err(error) => {
                    this.updates = UpdateState::Failed(error.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn update_available(&self) -> bool {
        matches!(self.updates, UpdateState::Available(_))
    }

    pub(super) fn update_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self
            .host
            .connection
            .threads
            .iter()
            .filter(|thread| thread.status.busy())
            .count();
        let (note, color) = match &self.updates {
            UpdateState::Idle => (
                "Updates install from signed GitHub releases.".to_owned(),
                colors().subtle,
            ),
            UpdateState::Checking => ("Checking for updates…".to_owned(), colors().subtle),
            UpdateState::Current => (
                "No newer signed release is published.".to_owned(),
                colors().subtle,
            ),
            UpdateState::Available(offer) => {
                let note = if busy > 0 {
                    "Finish or stop running threads first; the host restarts to update.".to_owned()
                } else {
                    format!(
                        "Plait {} is available. Installing restarts Plait and its host.",
                        offer.version
                    )
                };
                (note, colors().muted)
            }
            UpdateState::Installing(offer) => (
                format!("Installing Plait {}…", offer.version),
                colors().muted,
            ),
            UpdateState::Failed(error) => (error.clone(), colors().amber),
        };
        let action = match &self.updates {
            UpdateState::Available(offer) => toned_button(
                "install-update",
                format!("Install {} and Restart", offer.version),
                Tone::Primary,
            )
            .on_activate(cx, |this, window, cx| this.install_update(window, cx))
            .into_any_element(),
            UpdateState::Checking | UpdateState::Installing(_) => div().into_any_element(),
            _ => button("check-updates", "Check for Updates")
                .on_activate(cx, |this, _, cx| this.check_for_updates(false, cx))
                .into_any_element(),
        };
        column()
            .gap_1()
            .child(row().gap_2().child(action))
            .child(tag("update-status-note", note, color))
    }
}
