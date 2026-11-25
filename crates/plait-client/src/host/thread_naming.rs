//! Host-owned naming preferences; no naming request runs on the desktop.
use super::{Error, ModelChoice};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub model: Option<ModelChoice>,
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    GetThreadNaming,
    SetThreadNaming {
        enabled: bool,
        model: Option<ModelChoice>,
    },
}

#[derive(Default)]
pub struct State {
    pub epoch: Option<u64>,
    pub saved: Option<Settings>,
    pub busy: bool,
    pub unsupported: bool,
    pub note: Option<String>,
}

impl State {
    pub fn sync(&mut self, epoch: u64) {
        if self.epoch != Some(epoch) {
            *self = Self {
                epoch: Some(epoch),
                ..Self::default()
            };
        }
    }

    pub fn complete(&mut self, epoch: u64, result: Result<Settings, Error>) {
        if self.epoch != Some(epoch) {
            return;
        }
        self.busy = false;
        match result {
            Ok(value) if !value.enabled || value.model.is_some() => {
                self.saved = Some(value);
                self.unsupported = false;
                self.note = None;
            }
            Ok(_) => {
                self.note =
                    Some("The host returned invalid naming settings. Update the host.".into())
            }
            Err(Error::Rejected(error))
                if error.code == "invalid_request" && self.saved.is_none() =>
            {
                self.unsupported = true;
                self.note = Some("Update this host to configure thread naming.".into());
            }
            Err(error) => self.note = Some(format!("Naming settings were not confirmed. {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_clears_saved_settings_and_rejects_old_results() {
        let mut state = State::default();
        state.sync(1);
        state.complete(1, Ok(Settings::default()));
        assert!(state.saved.is_some());
        state.sync(2);
        state.busy = true;
        state.complete(1, Ok(Settings::default()));
        assert!(state.saved.is_none());
        assert!(state.busy);
        state.complete(2, Ok(Settings::default()));
        assert!(state.saved.is_some());
        assert!(!state.busy);
    }

    #[test]
    fn unconfirmed_save_keeps_the_last_confirmed_settings() {
        let mut state = State::default();
        state.sync(1);
        state.complete(1, Ok(Settings::default()));
        state.complete(1, Err(Error::Transport("Offline".into())));
        assert_eq!(state.saved, Some(Settings::default()));
        assert!(state.note.unwrap().contains("not confirmed"));
    }

    #[test]
    fn requests_do_not_change_existing_status_or_thread_wire_shapes() {
        assert_eq!(
            serde_json::to_value(Request::GetThreadNaming).unwrap(),
            serde_json::json!({"op":"get_thread_naming"})
        );
        assert_eq!(
            serde_json::to_value(Request::SetThreadNaming {
                enabled: false,
                model: None
            })
            .unwrap(),
            serde_json::json!({"op":"set_thread_naming","enabled":false,"model":null})
        );
        assert!(
            serde_json::from_value::<Settings>(
                serde_json::json!({"enabled":false,"model":null,"extra":1})
            )
            .is_err()
        );
    }
}
