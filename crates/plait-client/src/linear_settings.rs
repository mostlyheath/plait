//! The desktop's Linear account state, free of GPUI.
//!
//! The host owns the key and every Linear request. This model only tracks
//! what the person sees: whether the host supports Linear, who is signed in,
//! and the progress of a save or sign-out. Each request carries the
//! generation it started under; a reply from an older generation, such as one
//! that lands after a reconnect or after the person moved on, is discarded.
//! Messages are fixed text, so neither a host error nor a submitted key can
//! appear in them.

use crate::host::linear::{AuthState, AuthStatus, Binding, ProjectBinding, Rejection, Teams};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    Loading,
    /// The host predates the Linear operations.
    OlderHost,
    SignedOut,
    SignedIn {
        workspace: String,
        viewer: String,
    },
    /// Linear no longer accepts the saved key.
    KeyRejected,
}

/// What is being sent to the host right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pending {
    #[default]
    None,
    Loading,
    Saving,
    SigningOut,
}

#[derive(Debug, Default)]
pub struct Account {
    pub auth: Auth,
    pub pending: Pending,
    /// The key field is open while signed in (Replace key).
    pub replacing: bool,
    /// Sign out asked for confirmation and is waiting for an answer.
    pub confirm_sign_out: bool,
    pub note: Option<String>,
    /// The note explains a refusal rather than confirming a change.
    pub failed: bool,
    generation: u64,
    epoch: Option<u64>,
}

impl Account {
    /// Starts over for a new host connection. Returns whether anything changed.
    pub fn connect(&mut self, epoch: u64) -> bool {
        if self.epoch == Some(epoch) {
            return false;
        }
        *self = Self {
            generation: self.generation.wrapping_add(1),
            epoch: Some(epoch),
            ..Self::default()
        };
        true
    }

    /// Whether the status should be fetched: connected and nothing known yet.
    pub fn needs_load(&self) -> bool {
        self.auth == Auth::Loading && self.pending == Pending::None && self.note.is_none()
    }

    fn begin(&mut self, pending: Pending) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = pending;
        self.note = None;
        self.failed = false;
        self.generation
    }

    pub fn start_load(&mut self) -> u64 {
        self.begin(Pending::Loading)
    }

    /// A save shows the field's outcome; an empty key is not sent.
    pub fn start_save(&mut self, key: &str) -> Option<u64> {
        if key.trim().is_empty() || self.pending != Pending::None {
            return None;
        }
        Some(self.begin(Pending::Saving))
    }

    pub fn ask_sign_out(&mut self) {
        self.confirm_sign_out = true;
        self.replacing = false;
    }

    pub fn cancel_sign_out(&mut self) {
        self.confirm_sign_out = false;
    }

    pub fn start_sign_out(&mut self) -> Option<u64> {
        if !self.confirm_sign_out || self.pending != Pending::None {
            return None;
        }
        self.confirm_sign_out = false;
        Some(self.begin(Pending::SigningOut))
    }

    pub fn replace_key(&mut self, open: bool) {
        self.replacing = open;
        self.confirm_sign_out = false;
        self.note = None;
    }

    /// Applies a status reply from a load, save, or sign-out. Returns whether
    /// it was current; a stale reply changes nothing.
    pub fn reply(&mut self, generation: u64, result: Result<AuthStatus, Rejection>) -> bool {
        if generation != self.generation || self.pending == Pending::None {
            return false;
        }
        let pending = std::mem::take(&mut self.pending);
        match result {
            Ok(status) => {
                self.replacing = false;
                self.auth = auth(&status);
                self.note = match (pending, &self.auth) {
                    (Pending::Saving, Auth::SignedIn { .. }) => {
                        Some("Key saved on the host.".into())
                    }
                    (Pending::SigningOut, Auth::SignedOut) => Some("Signed out of Linear.".into()),
                    _ => None,
                };
            }
            // The probe is the first load: an unknown operation means an older host.
            Err(Rejection::NeedsNewerHost) => self.auth = Auth::OlderHost,
            Err(error) => {
                self.failed = true;
                self.note = Some(message(pending, error));
                if pending == Pending::Loading {
                    self.auth = Auth::SignedOut;
                }
            }
        }
        true
    }
}

fn auth(status: &AuthStatus) -> Auth {
    match status.status {
        AuthState::SignedOut => Auth::SignedOut,
        AuthState::KeyRejected => Auth::KeyRejected,
        AuthState::SignedIn => Auth::SignedIn {
            workspace: status
                .workspace_name
                .clone()
                .unwrap_or_else(|| "your workspace".into()),
            viewer: status.viewer_name.clone().unwrap_or_else(|| "you".into()),
        },
    }
}

/// What the project's Linear section is sending to the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TeamPending {
    #[default]
    None,
    Loading,
    Saving,
    Refreshing,
}

/// One project's Linear team: its binding and the teams it can choose from.
/// Like `Account`, a reply from an older generation changes nothing, and a
/// new connection or another project starts over.
#[derive(Debug, Default)]
pub struct ProjectTeam {
    pub checkout: Option<PathBuf>,
    pub binding: Option<Binding>,
    pub teams: Option<Teams>,
    pub pending: TeamPending,
    pub note: Option<String>,
    pub failed: bool,
    generation: u64,
    epoch: Option<u64>,
}

impl ProjectTeam {
    /// Points the state at `checkout` on connection `epoch`, starting over
    /// when either changed. Returns whether it started over.
    pub fn target(&mut self, epoch: u64, checkout: &Path) -> bool {
        if self.epoch == Some(epoch) && self.checkout.as_deref() == Some(checkout) {
            return false;
        }
        *self = Self {
            checkout: Some(checkout.to_owned()),
            generation: self.generation.wrapping_add(1),
            epoch: Some(epoch),
            ..Self::default()
        };
        true
    }

    pub fn needs_load(&self) -> bool {
        self.teams.is_none() && self.pending == TeamPending::None && self.note.is_none()
    }

    fn begin(&mut self, pending: TeamPending) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = pending;
        self.note = None;
        self.failed = false;
        self.generation
    }

    pub fn start_load(&mut self) -> u64 {
        self.begin(TeamPending::Loading)
    }

    pub fn start_refresh(&mut self) -> Option<u64> {
        (self.pending == TeamPending::None).then(|| self.begin(TeamPending::Refreshing))
    }

    /// Binding to the team already bound, or while busy, sends nothing.
    pub fn start_bind(&mut self, team_id: Option<&str>) -> Option<u64> {
        let current = self
            .binding
            .as_ref()
            .map(|binding| binding.team_id.as_str());
        if self.pending != TeamPending::None || current == team_id {
            return None;
        }
        Some(self.begin(TeamPending::Saving))
    }

    fn accept(&mut self, generation: u64) -> Option<TeamPending> {
        if generation != self.generation || self.pending == TeamPending::None {
            return None;
        }
        Some(std::mem::take(&mut self.pending))
    }

    fn refuse(&mut self, pending: TeamPending, error: Rejection) {
        self.failed = true;
        self.note = Some(team_message(pending, error));
    }

    /// The binding and team list read together when the section opens.
    pub fn loaded(
        &mut self,
        generation: u64,
        result: Result<(ProjectBinding, Teams), Rejection>,
    ) -> bool {
        let Some(pending) = self.accept(generation) else {
            return false;
        };
        match result {
            Ok((project, teams)) => {
                self.binding = project.binding;
                self.teams = Some(teams);
            }
            Err(error) => self.refuse(pending, error),
        }
        true
    }

    pub fn refreshed(&mut self, generation: u64, result: Result<Teams, Rejection>) -> bool {
        let Some(pending) = self.accept(generation) else {
            return false;
        };
        match result {
            Ok(teams) => self.teams = Some(teams),
            Err(error) => self.refuse(pending, error),
        }
        true
    }

    /// The binding only changes once the host confirms it.
    pub fn bound(&mut self, generation: u64, result: Result<ProjectBinding, Rejection>) -> bool {
        let Some(pending) = self.accept(generation) else {
            return false;
        };
        match result {
            Ok(project) => {
                self.note = Some(match &project.binding {
                    Some(binding) => format!(
                        "Linked to {}.",
                        binding.team_name.as_deref().unwrap_or("the team")
                    ),
                    None => "Unlinked from Linear.".into(),
                });
                self.binding = project.binding;
            }
            Err(error) => self.refuse(pending, error),
        }
        true
    }
}

fn team_message(pending: TeamPending, error: Rejection) -> String {
    match (pending, error) {
        (_, Rejection::SignedOut | Rejection::KeyRejected) => {
            "Sign in to Linear again in Settings → Linear.".into()
        }
        (TeamPending::Saving, Rejection::Offline) => {
            "Linear could not be reached, so the link was not changed.".into()
        }
        (TeamPending::Saving, Rejection::Other | Rejection::Conflict | Rejection::NotFound) => {
            "The host could not change the link. Nothing was changed.".into()
        }
        (_, error) => message(Pending::Loading, error),
    }
}

/// Fixed text for a refused request. Host messages are never shown here.
pub fn message(pending: Pending, error: Rejection) -> String {
    let nothing_saved = pending == Pending::Saving;
    match error {
        Rejection::NeedsNewerHost => "Linear needs a newer host. Update Plait's host.".into(),
        Rejection::KeyRejected => "Linear did not accept this key. Check it and try again.".into(),
        Rejection::Offline if nothing_saved => {
            "Linear could not be reached, so nothing was saved. Try again when online.".into()
        }
        Rejection::Offline => "Linear could not be reached. Try again when online.".into(),
        Rejection::RateLimited(Some(seconds)) => {
            format!("Linear is limiting requests. Try again in {seconds} seconds.")
        }
        Rejection::RateLimited(None) => "Linear is limiting requests. Try again shortly.".into(),
        Rejection::Conflict | Rejection::NotFound | Rejection::SignedOut | Rejection::Other => {
            if nothing_saved {
                "The host could not save the key. Nothing was changed.".into()
            } else {
                "The host could not complete the Linear request.".into()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: AuthState) -> AuthStatus {
        AuthStatus {
            signed_in: state == AuthState::SignedIn,
            status: state,
            workspace_id: (state == AuthState::SignedIn).then(|| "ORG".into()),
            workspace_name: (state == AuthState::SignedIn).then(|| "Acme".into()),
            workspace_url_key: None,
            viewer_id: None,
            viewer_name: (state == AuthState::SignedIn).then(|| "Alex".into()),
        }
    }

    fn connected() -> Account {
        let mut account = Account::default();
        account.connect(1);
        account
    }

    #[test]
    fn a_load_shows_who_is_signed_in() {
        let mut account = connected();
        assert!(account.needs_load());
        let load = account.start_load();
        assert!(!account.needs_load());
        assert!(account.reply(load, Ok(status(AuthState::SignedIn))));
        assert_eq!(
            account.auth,
            Auth::SignedIn {
                workspace: "Acme".into(),
                viewer: "Alex".into()
            }
        );
        assert_eq!(account.pending, Pending::None);
    }

    #[test]
    fn an_older_host_is_reported_once() {
        let mut account = connected();
        let load = account.start_load();
        account.reply(load, Err(Rejection::NeedsNewerHost));
        assert_eq!(account.auth, Auth::OlderHost);
        assert!(!account.needs_load());
    }

    #[test]
    fn stale_replies_change_nothing() {
        let mut account = connected();
        let old = account.start_load();
        // A reconnect starts over; the old reply must not land.
        assert!(account.connect(2));
        assert!(!account.reply(old, Ok(status(AuthState::SignedIn))));
        assert_eq!(account.auth, Auth::Loading);
        // A newer request supersedes an older one on the same connection.
        let first = account.start_load();
        account.pending = Pending::None;
        let second = account.start_save("key").unwrap();
        assert!(!account.reply(first, Ok(status(AuthState::SignedOut))));
        assert!(account.reply(second, Ok(status(AuthState::SignedIn))));
        // The same connection does not reset.
        assert!(!account.connect(2));
    }

    #[test]
    fn saving_needs_a_key_and_one_request_at_a_time() {
        let mut account = connected();
        let load = account.start_load();
        account.reply(load, Ok(status(AuthState::SignedOut)));
        assert_eq!(account.start_save("   "), None);
        let save = account.start_save("lin_api_x").unwrap();
        assert_eq!(account.start_save("again"), None);
        account.reply(save, Ok(status(AuthState::SignedIn)));
        assert_eq!(account.note.as_deref(), Some("Key saved on the host."));
    }

    #[test]
    fn a_refused_save_keeps_the_state_and_explains() {
        let mut account = connected();
        let load = account.start_load();
        account.reply(load, Ok(status(AuthState::SignedOut)));
        let save = account.start_save("bad").unwrap();
        account.reply(save, Err(Rejection::KeyRejected));
        assert_eq!(account.auth, Auth::SignedOut);
        assert!(account.failed);
        assert_eq!(
            account.note.as_deref(),
            Some("Linear did not accept this key. Check it and try again.")
        );
        let save = account.start_save("later").unwrap();
        account.reply(save, Err(Rejection::Offline));
        assert!(
            account
                .note
                .as_deref()
                .unwrap()
                .contains("nothing was saved")
        );
    }

    #[test]
    fn sign_out_needs_confirmation() {
        let mut account = connected();
        let load = account.start_load();
        account.reply(load, Ok(status(AuthState::SignedIn)));
        assert_eq!(account.start_sign_out(), None, "not without asking");
        account.ask_sign_out();
        account.cancel_sign_out();
        assert_eq!(account.start_sign_out(), None, "not after Cancel");
        account.ask_sign_out();
        let out = account.start_sign_out().unwrap();
        account.reply(out, Ok(status(AuthState::SignedOut)));
        assert_eq!(account.auth, Auth::SignedOut);
        assert_eq!(account.note.as_deref(), Some("Signed out of Linear."));
    }

    #[test]
    fn a_rejected_saved_key_is_its_own_state() {
        let mut account = connected();
        let load = account.start_load();
        account.reply(load, Ok(status(AuthState::KeyRejected)));
        assert_eq!(account.auth, Auth::KeyRejected);
    }

    fn teams(ids: &[&str]) -> Teams {
        serde_json::from_value(serde_json::json!({
            "teams": ids.iter().map(|id| serde_json::json!({"id": id, "key": id.to_uppercase(), "name": format!("Team {id}"), "states": []})).collect::<Vec<_>>(),
            "teams_complete": true,
            "cached": false, "stale": false, "cached_at": 0, "cache_age_seconds": 0, "partial": false, "warning": null
        }))
        .unwrap()
    }

    fn project(team: Option<&str>) -> ProjectBinding {
        serde_json::from_value(serde_json::json!({
            "checkout": "/repo",
            "binding": team.map(|id| serde_json::json!({"workspace_id": "ORG", "team_id": id, "team_key": "K", "team_name": format!("Team {id}"), "updated_at": 1})),
        }))
        .unwrap()
    }

    #[test]
    fn a_project_loads_its_binding_and_teams_and_binds_once_confirmed() {
        let mut team = ProjectTeam::default();
        assert!(team.target(1, Path::new("/repo")));
        assert!(team.needs_load());
        let load = team.start_load();
        assert!(team.loaded(load, Ok((project(None), teams(&["a", "b"])))));
        assert!(team.binding.is_none());
        let bind = team.start_bind(Some("a")).unwrap();
        // Not yet confirmed: the binding is unchanged until the host replies.
        assert!(team.binding.is_none());
        assert_eq!(team.start_bind(Some("b")), None, "one change at a time");
        team.bound(bind, Ok(project(Some("a"))));
        assert_eq!(team.binding.as_ref().unwrap().team_id, "a");
        assert_eq!(team.note.as_deref(), Some("Linked to Team a."));
        assert_eq!(team.start_bind(Some("a")), None, "already bound");
        let unlink = team.start_bind(None).unwrap();
        team.bound(unlink, Ok(project(None)));
        assert!(team.binding.is_none());
    }

    #[test]
    fn another_project_or_connection_starts_over_and_drops_old_replies() {
        let mut team = ProjectTeam::default();
        team.target(1, Path::new("/repo"));
        let load = team.start_load();
        assert!(team.target(1, Path::new("/other")));
        assert!(!team.loaded(load, Ok((project(Some("a")), teams(&["a"])))));
        assert!(team.teams.is_none());
        assert!(!team.target(1, Path::new("/other")));
        assert!(team.target(2, Path::new("/other")));
    }

    #[test]
    fn a_refused_link_keeps_the_old_one() {
        let mut team = ProjectTeam::default();
        team.target(1, Path::new("/repo"));
        let load = team.start_load();
        team.loaded(load, Ok((project(Some("a")), teams(&["a", "b"]))));
        let bind = team.start_bind(Some("b")).unwrap();
        team.bound(bind, Err(Rejection::Offline));
        assert_eq!(team.binding.as_ref().unwrap().team_id, "a");
        assert!(team.failed);
        assert!(team.note.as_deref().unwrap().contains("not changed"));
    }

    #[test]
    fn messages_are_fixed_text() {
        let key = "lin_api_SECRET";
        for pending in [Pending::Loading, Pending::Saving, Pending::SigningOut] {
            for error in [
                Rejection::NeedsNewerHost,
                Rejection::KeyRejected,
                Rejection::Offline,
                Rejection::RateLimited(Some(12)),
                Rejection::RateLimited(None),
                Rejection::Conflict,
                Rejection::NotFound,
                Rejection::SignedOut,
                Rejection::Other,
            ] {
                assert!(!message(pending, error).contains(key));
            }
        }
        assert!(message(Pending::Loading, Rejection::RateLimited(Some(12))).contains("12 seconds"));
    }
}
