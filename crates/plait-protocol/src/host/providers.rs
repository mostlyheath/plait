use serde::{Deserialize, Serialize, Serializer};

/// The most model IDs one custom endpoint can list.
pub const MODEL_ID_LIMIT: usize = 200;
/// The longest key or sign-in answer the host accepts, in bytes.
pub const SECRET_LIMIT: usize = 8192;

/// Text the user typed that must reach the host and nothing else: an API key
/// or a pasted sign-in code. It is overwritten when dropped, prints as a
/// placeholder, and cannot be cloned or read back except to serialize it.
#[derive(Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: String) -> Self {
        Self(text)
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// Overwrite text before its memory is returned to the allocator.
pub fn wipe(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.fill(0);
    // Keep the overwrite from being removed as a dead store.
    std::hint::black_box(&bytes);
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Every provider operation. Field order follows the documented examples.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    ReloadProviders,
    ListProviders,
    StartSignIn {
        provider: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<String>,
    },
    /// Run Codex's or Claude Code's own sign-in on the host. The reply and the
    /// status, answer, and cancel operations are a provider sign-in's, with
    /// provider `harness:<id>`.
    StartHarnessSignIn {
        harness: super::harness::HarnessId,
    },
    SignInStatus {
        sign_in_id: String,
    },
    AnswerSignIn {
        sign_in_id: String,
        input: Secret,
    },
    CancelSignIn {
        sign_in_id: String,
    },
    SetProviderKey {
        provider: String,
        key: Secret,
    },
    SignOutProvider {
        provider: String,
    },
    AddEndpoint {
        id: String,
        name: String,
        base_url: String,
        model_ids: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Secret>,
    },
    /// A field left out keeps its saved value.
    UpdateEndpoint {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Secret>,
    },
    RemoveEndpoint {
        id: String,
    },
    /// Check an address that is not saved. The key is used once, not stored.
    #[serde(rename = "check_endpoint")]
    CheckAddress {
        base_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Secret>,
    },
    /// Check a saved endpoint with its saved key and headers.
    #[serde(rename = "check_endpoint")]
    CheckSaved {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Ships with Pi.
    #[default]
    Builtin,
    /// Declared in Pi's `models.json`.
    Custom,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Credential {
    #[default]
    None,
    SignIn,
    ApiKey,
    /// An environment variable, a key in `models.json`, or cloud credentials.
    /// The host does not manage it and cannot remove it.
    External,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SignInMethod {
    pub name: String,
    /// A plan such as Claude Pro or ChatGPT Plus, not an ordinary account.
    #[serde(default)]
    pub subscription: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct KeyMethod {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    /// Shown without credentials; `None` when the host could not parse it.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub model_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: ProviderKind,
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub credential: Credential,
    #[serde(default)]
    pub model_count: u32,
    #[serde(default)]
    pub sign_in: Option<SignInMethod>,
    #[serde(default)]
    pub api_key: Option<KeyMethod>,
    #[serde(default)]
    pub endpoint: Option<Endpoint>,
}

/// What a provider row can offer. Derived from the host's reply only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAction {
    SignIn,
    EnterKey,
    SignOut,
    Edit,
    Check,
    Remove,
}

impl Provider {
    pub fn is_custom(&self) -> bool {
        self.kind == ProviderKind::Custom
    }

    /// How the provider is authenticated now, in words.
    pub fn credential_label(&self) -> &'static str {
        match self.credential {
            Credential::SignIn
                if self
                    .sign_in
                    .as_ref()
                    .is_some_and(|method| method.subscription) =>
            {
                "Signed in with a subscription"
            }
            Credential::SignIn => "Signed in with an account",
            Credential::ApiKey => "API key saved",
            Credential::External if self.is_custom() => "Usable; no key saved in Plait",
            Credential::External => "Configured outside Plait",
            Credential::None | Credential::Other if self.authenticated => "Signed in",
            Credential::None | Credential::Other if self.is_custom() => "No usable key",
            Credential::None | Credential::Other => "Not signed in",
        }
    }

    pub fn model_label(&self) -> String {
        match self.model_count {
            1 => "1 model".into(),
            count => format!("{count} models"),
        }
    }

    /// The actions that apply to this provider, in the order a row shows them.
    pub fn actions(&self) -> Vec<ProviderAction> {
        if self.is_custom() {
            // A custom server's key is part of its Edit form.
            return vec![
                ProviderAction::Check,
                ProviderAction::Edit,
                ProviderAction::Remove,
            ];
        }
        let mut actions = Vec::new();
        if self.sign_in.is_some() && self.credential != Credential::SignIn {
            actions.push(ProviderAction::SignIn);
        }
        if self.api_key.is_some() {
            actions.push(ProviderAction::EnterKey);
        }
        // An external credential is not the host's to remove.
        if matches!(self.credential, Credential::SignIn | Credential::ApiKey) {
            actions.push(ProviderAction::SignOut);
        }
        actions
    }

    /// Whether Plait can sign in to it at all.
    pub fn connectable(&self) -> bool {
        self.sign_in.is_some() || self.api_key.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ProvidersReply {
    pub providers: Vec<Provider>,
    /// A reload that succeeded with a caveat, such as a login shell the host could not read.
    #[serde(default)]
    pub warning: Option<String>,
}

/// Shown in Settings → Providers when a reload could not read the login shell.
pub const STALE_ENVIRONMENT: &str =
    "Couldn't read your shell's environment; keys from it may be out of date.";

impl ProvidersReply {
    /// The reload's caveat in plain words. A warning this desktop does not know is not shown.
    pub fn warning_text(&self) -> Option<&'static str> {
        match self.warning.as_deref()? {
            "login_environment_unavailable" => Some(STALE_ENVIRONMENT),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ProviderReply {
    pub provider: Provider,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RemovedReply {
    pub removed: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckReason {
    Timeout,
    Unreachable,
    Unauthorized,
    NotFound,
    HttpError,
    InvalidResponse,
    #[serde(other)]
    Other,
}

/// The result of one `GET BASE_URL/models`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct EndpointCheck {
    pub reachable: bool,
    #[serde(default)]
    pub reason: Option<CheckReason>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(default)]
    pub truncated: bool,
}

impl EndpointCheck {
    /// The outcome in one sentence.
    pub fn summary(&self) -> String {
        let status = self
            .status
            .map(|status| format!(" (HTTP {status})"))
            .unwrap_or_default();
        if self.reachable {
            let more = if self.truncated {
                " The first 500 are listed."
            } else {
                ""
            };
            return match self.model_ids.len() {
                0 => "The server answered but reported no models. Enter model IDs by hand.".into(),
                1 => format!("The server answered and reports 1 model.{more}"),
                count => format!("The server answered and reports {count} models.{more}"),
            };
        }
        let reason = match self.reason {
            Some(CheckReason::Timeout) => "The server did not answer within 5 seconds.",
            Some(CheckReason::Unreachable) => "The server could not be reached at that address.",
            Some(CheckReason::Unauthorized) => "The server refused the key.",
            Some(CheckReason::NotFound) => {
                "The server has no /models at that address. Check the base URL, which usually ends in /v1."
            }
            Some(CheckReason::HttpError) => {
                "The server answered with an error or a redirect, which is not followed."
            }
            Some(CheckReason::InvalidResponse) => {
                "The server's answer was not an OpenAI-compatible model list."
            }
            Some(CheckReason::Other) | None => "The check did not succeed.",
        };
        format!("{reason}{status}")
    }
}
