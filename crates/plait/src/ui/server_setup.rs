use super::*;
use plait_core::update::setup::{self, Outcome};

/// First-launch setup of a missing local server. Runs at most once on its own
/// per launch, after a failed connection; Retry runs it again.
#[derive(Clone, Default)]
pub enum ServerSetup {
    #[default]
    NotTried,
    Running,
    /// Nothing to say: a server existed, setup finished, or this is a source build.
    Settled,
    /// Setup cannot run from here; what the person can do instead.
    Guidance(&'static str),
    Failed(String),
}

impl Plait {
    /// Called when the host does not answer. A server that exists is never
    /// replaced here; setup only installs one that is missing.
    pub(super) fn offer_server_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Setup installs this machine’s server only.
        if self.host_is_local() && matches!(self.server_setup, ServerSetup::NotTried) {
            self.run_server_setup(window, cx);
        }
    }

    pub(super) fn run_server_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.server_setup, ServerSetup::Running) {
            return;
        }
        // Test scenes stay offline, and only macOS has a server to set up.
        if cfg!(test) || !cfg!(target_os = "macos") {
            self.server_setup = ServerSetup::Settled;
            return;
        }
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            self.server_setup =
                ServerSetup::Failed("No home folder to set up the server in.".into());
            cx.notify();
            return;
        };
        self.server_setup = ServerSetup::Running;
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { setup::ensure(&home) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.server_setup = match result {
                    Ok(Outcome::Started) => {
                        this.server_setup = ServerSetup::Settled;
                        this.reconnect_host(window, cx);
                        return;
                    }
                    Ok(Outcome::Existing) => ServerSetup::Settled,
                    Ok(Outcome::Refused(refusal)) => refusal
                        .guidance()
                        .map_or(ServerSetup::Settled, ServerSetup::Guidance),
                    Err(error) => ServerSetup::Failed(format!("Server setup failed. {error}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether setup has something to show in place of the connection error.
    pub(super) fn server_setup_visible(&self) -> bool {
        !self.host.connection.connected
            && matches!(
                self.server_setup,
                ServerSetup::Running | ServerSetup::Guidance(_) | ServerSetup::Failed(_)
            )
    }

    /// Setup progress, guidance, or failure with Retry; `None` when there is none.
    pub(super) fn server_setup_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.server_setup_visible() {
            return None;
        }
        let (color, text, retry) = match &self.server_setup {
            ServerSetup::Running => (
                colors().blue,
                "Setting up Plait's server on this Mac…".to_owned(),
                false,
            ),
            ServerSetup::Guidance(text) => (colors().amber, (*text).to_owned(), true),
            ServerSetup::Failed(error) => (colors().red, error.clone(), true),
            ServerSetup::NotTried | ServerSetup::Settled => return None,
        };
        Some(
            row()
                .w_full()
                .gap_2()
                .text_size(px(11.))
                .child(dot(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(if retry { color } else { colors().muted }))
                        .child(a11y_text("server-setup-status", text)),
                )
                .when(retry, |el| {
                    el.child(
                        toned_button("server-setup-retry", "Retry", Tone::Filled)
                            .on_activate(cx, |this, window, cx| this.run_server_setup(window, cx)),
                    )
                })
                .into_any_element(),
        )
    }
}
