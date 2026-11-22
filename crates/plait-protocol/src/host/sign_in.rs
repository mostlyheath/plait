use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    /// Nothing to show yet.
    Starting,
    /// The user must act.
    Waiting,
    /// The host is exchanging the result.
    Finishing,
    SignedIn,
    Failed,
    Cancelled,
    Expired,
    /// A state from a newer host.
    #[serde(other)]
    Other,
}

impl SignInState {
    /// Still in progress: keep reading `sign_in_status`.
    pub fn pending(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Waiting | Self::Finishing | Self::Other
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// An authorization code or the final redirect address.
    Code,
    /// One of `choices`, answered with its ID.
    Choice,
    Text,
    Secret,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    #[serde(default)]
    pub label: String,
}

fn required() -> bool {
    true
}

/// The one question a sign-in is asking.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SignInInput {
    pub kind: InputKind,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    /// False only for a code the browser normally delivers by itself.
    #[serde(default = "required")]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct SignIn {
    pub id: String,
    pub provider: String,
    pub state: SignInState,
    /// A one-time address that can hold the flow's state. Open it; never log or save it.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub user_code: Option<String>,
    #[serde(default)]
    pub input: Option<SignInInput>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub expires_in_seconds: u64,
    #[serde(default)]
    pub error: Option<Failure>,
}

// The address and code are one-time values of a pending sign-in: keep them out of logs.
impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("state", &self.state)
            .field("url", &self.url.as_ref().map(|_| "…"))
            .field("user_code", &self.user_code.as_ref().map(|_| "…"))
            .field("input", &self.input.as_ref().map(|input| input.kind))
            .field("expires_in_seconds", &self.expires_in_seconds)
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SignInReply {
    pub sign_in: SignIn,
}
