//! A subscription sign-in as the host reports it, and what a client shows and
//! offers for each state. docs/HOST.md "Subscription sign-in" is the contract.
//!
//! [`view`] is a pure function, so every state and input kind is tested
//! without a host or a window.

pub use plait_protocol::host::sign_in::{
    Choice, Failure, InputKind, SignIn, SignInInput, SignInReply, SignInState,
};

/// What a state means to the person looking at it. Shown as words and a color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The host is working.
    Progress,
    /// The person must act.
    Attention,
    Success,
    Failure,
    Neutral,
}

/// Where "Open in browser" goes, and the server name to show before it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    pub url: String,
    /// The server the browser will contact, with its port if one is given.
    pub host: String,
    /// False for plain `http`.
    pub encrypted: bool,
}

/// The server an address leads to, or `None` when it is not a plain web address.
pub fn destination(url: &str) -> Option<Destination> {
    let (encrypted, rest) = match url.strip_prefix("https://") {
        Some(rest) => (true, rest),
        None => (false, url.strip_prefix("http://")?),
    };
    if url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return None;
    }
    // Browsers read a backslash as a path separator in web addresses.
    let authority = rest.split(['/', '\\', '?', '#']).next()?;
    // Anything before the last `@` is a user name, not the server.
    let host = authority.rsplit('@').next()?;
    if host.is_empty() || host.starts_with(':') {
        return None;
    }
    Some(Destination {
        url: url.to_owned(),
        host: host.to_lowercase(),
        encrypted,
    })
}

/// The input a sign-in asks for, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// One line of text, sent as the answer.
    Field {
        label: String,
        placeholder: String,
        /// Draw the text hidden, with a reveal toggle.
        masked: bool,
        /// The flow normally finishes without it; offer it anyway.
        optional: bool,
        submit: &'static str,
    },
    /// One button per choice; the answer is the choice's ID.
    Choice { label: String, choices: Vec<Choice> },
}

/// Everything the sign-in panel shows and offers for one host state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The state in words.
    pub status: String,
    pub tone: Tone,
    /// The flow's own instruction, or the reason it ended.
    pub detail: Option<String>,
    pub destination: Option<Destination>,
    /// The host sent an address that cannot be opened.
    pub unusable_address: bool,
    /// A device code to type on the provider's page.
    pub user_code: Option<String>,
    pub prompt: Option<Prompt>,
    /// Time left, such as "9 min 58 s left".
    pub remaining: Option<String>,
    /// Read `sign_in_status` again in about a second.
    pub poll: bool,
    pub can_cancel: bool,
    /// Offer to start the sign-in again.
    pub can_retry: bool,
    /// Label of the control that closes the panel, once the flow has ended.
    pub close: Option<&'static str>,
    /// The catalog changed: reload status and the provider list.
    pub signed_in: bool,
}

pub fn remaining(seconds: u64) -> String {
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds} s left"),
        (minutes, 0) => format!("{minutes} min left"),
        (minutes, seconds) => format!("{minutes} min {seconds} s left"),
    }
}

fn text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn prompt(input: &SignInInput) -> Prompt {
    let message = text(Some(&input.message));
    let placeholder = text(input.placeholder.as_deref()).unwrap_or_default();
    let field = |fallback: &str, masked, optional, submit| Prompt::Field {
        label: message.clone().unwrap_or_else(|| fallback.into()),
        placeholder: placeholder.clone(),
        masked,
        optional,
        submit,
    };
    match input.kind {
        InputKind::Code => field(
            "Paste the authorization code or the final address:",
            true,
            !input.required,
            "Submit code",
        ),
        InputKind::Secret => field(
            "Enter the value the provider asks for:",
            true,
            false,
            "Submit",
        ),
        InputKind::Text | InputKind::Other => {
            field("Answer the provider's question:", false, false, "Submit")
        }
        InputKind::Choice => Prompt::Choice {
            label: message.unwrap_or_else(|| "Choose how to sign in:".into()),
            choices: input.choices.clone(),
        },
    }
}

fn waiting_status(sign_in: &SignIn, opens: bool) -> &'static str {
    let asks = sign_in.input.as_ref().is_some_and(|input| input.required);
    if sign_in.user_code.is_some() {
        "Enter the code on the provider's page"
    } else if asks
        && matches!(
            sign_in.input.as_ref().map(|input| input.kind),
            Some(InputKind::Choice)
        )
    {
        "Choose how to sign in"
    } else if opens && asks {
        "Sign in in your browser, then paste the code here"
    } else if opens {
        "Waiting for you to sign in in your browser"
    } else if asks {
        "The sign-in needs an answer"
    } else {
        "Waiting for the provider"
    }
}

fn ended(status: &str, tone: Tone, detail: Option<String>, signed_in: bool) -> View {
    View {
        status: status.into(),
        tone,
        detail,
        destination: None,
        unusable_address: false,
        user_code: None,
        prompt: None,
        remaining: None,
        poll: false,
        can_cancel: false,
        can_retry: !signed_in,
        close: Some(if signed_in { "Done" } else { "Close" }),
        signed_in,
    }
}

fn working(status: &str, sign_in: &SignIn) -> View {
    View {
        status: status.into(),
        tone: Tone::Progress,
        detail: None,
        destination: None,
        unusable_address: false,
        user_code: None,
        prompt: None,
        remaining: Some(remaining(sign_in.expires_in_seconds)),
        poll: true,
        can_cancel: true,
        can_retry: false,
        close: None,
        signed_in: false,
    }
}

/// What to show and offer for a sign-in in the state the host last reported.
pub fn view(sign_in: &SignIn) -> View {
    match sign_in.state {
        SignInState::Starting => working("Starting the sign-in…", sign_in),
        SignInState::Finishing => working("Finishing the sign-in…", sign_in),
        SignInState::Other => View {
            tone: Tone::Attention,
            detail: Some(
                "This desktop does not know that state. Update Plait, or cancel and start again."
                    .into(),
            ),
            ..working("The host reported an unfamiliar sign-in state", sign_in)
        },
        SignInState::Waiting => {
            let destination = sign_in.url.as_deref().and_then(destination);
            View {
                status: waiting_status(sign_in, destination.is_some()).into(),
                tone: Tone::Attention,
                detail: text(sign_in.message.as_deref()),
                unusable_address: sign_in.url.is_some() && destination.is_none(),
                destination,
                user_code: text(sign_in.user_code.as_deref()),
                prompt: sign_in.input.as_ref().map(prompt),
                ..working("", sign_in)
            }
        }
        SignInState::SignedIn => ended(
            "Signed in. Its models are available now.",
            Tone::Success,
            None,
            true,
        ),
        SignInState::Failed => ended(
            "The sign-in failed",
            Tone::Failure,
            Some(
                sign_in
                    .error
                    .as_ref()
                    .and_then(|error| text(Some(&error.message)))
                    .unwrap_or_else(|| "The host gave no reason. Nothing was saved.".into()),
            ),
            false,
        ),
        SignInState::Cancelled => ended(
            "Sign-in cancelled. Nothing was saved.",
            Tone::Neutral,
            None,
            false,
        ),
        SignInState::Expired => ended(
            "The sign-in expired before it finished. Nothing was saved.",
            Tone::Attention,
            None,
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented example, verbatim.
    const WAITING: &str = r#"{"id":"HOST_UUID","provider":"anthropic","state":"waiting","url":"https://provider.example/authorize?...","user_code":null,"input":{"kind":"code","message":"Complete login in your browser, or paste the authorization code / redirect URL here:","placeholder":"http://localhost:53692/callback","required":false,"choices":[]},"message":"Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.","expires_in_seconds":598,"error":null}"#;

    fn parse(json: &str) -> SignIn {
        serde_json::from_str(json).unwrap()
    }

    fn in_state(state: &str) -> SignIn {
        parse(&format!(
            r#"{{"id":"s","provider":"anthropic","state":"{state}","url":null,"user_code":null,"input":null,"message":null,"expires_in_seconds":600,"error":null}}"#
        ))
    }

    fn asking(kind: &str, required: bool, url: Option<&str>) -> SignIn {
        let mut sign_in = in_state("waiting");
        sign_in.url = url.map(str::to_owned);
        sign_in.input = Some(
            serde_json::from_str(&format!(
                r#"{{"kind":"{kind}","message":"Question?","placeholder":null,"required":{required},"choices":[{{"id":"browser","label":"Browser"}},{{"id":"device","label":"Device code"}}]}}"#
            ))
            .unwrap(),
        );
        sign_in
    }

    #[test]
    fn the_documented_reply_is_read() {
        let reply: SignInReply =
            serde_json::from_str(&format!(r#"{{"sign_in":{WAITING}}}"#)).unwrap();
        let sign_in = reply.sign_in;
        assert_eq!(sign_in.id, "HOST_UUID");
        assert_eq!(sign_in.provider, "anthropic");
        assert_eq!(sign_in.state, SignInState::Waiting);
        assert_eq!(
            sign_in.url.as_deref(),
            Some("https://provider.example/authorize?...")
        );
        assert_eq!(sign_in.user_code, None);
        assert_eq!(sign_in.expires_in_seconds, 598);
        assert_eq!(sign_in.error, None);
        let input = sign_in.input.as_ref().unwrap();
        assert_eq!(input.kind, InputKind::Code);
        assert!(!input.required && input.choices.is_empty());
        assert_eq!(
            input.placeholder.as_deref(),
            Some("http://localhost:53692/callback")
        );
    }

    #[test]
    fn every_state_and_input_kind_is_read_and_unknown_ones_do_not_fail() {
        for (state, expected) in [
            ("starting", SignInState::Starting),
            ("waiting", SignInState::Waiting),
            ("finishing", SignInState::Finishing),
            ("signed_in", SignInState::SignedIn),
            ("failed", SignInState::Failed),
            ("cancelled", SignInState::Cancelled),
            ("expired", SignInState::Expired),
            ("verifying_device", SignInState::Other),
        ] {
            assert_eq!(in_state(state).state, expected);
        }
        for (kind, expected) in [
            ("code", InputKind::Code),
            ("choice", InputKind::Choice),
            ("text", InputKind::Text),
            ("secret", InputKind::Secret),
            ("passkey", InputKind::Other),
        ] {
            assert_eq!(asking(kind, true, None).input.unwrap().kind, expected);
        }
        // Alphabetical keys, an added field at each level, and a failure.
        let newer = parse(
            r#"{"attempt":2,"error":{"code":"permission_denied","message":"The provider denied the sign-in.","retryable":false},"expires_in_seconds":0,"id":"s","input":null,"message":null,"provider":"p","state":"failed","url":null,"user_code":null}"#,
        );
        assert_eq!(newer.error.unwrap().code, "permission_denied");
        // A sparse reply still loads; a missing `required` is treated as required.
        let sparse =
            parse(r#"{"id":"s","provider":"p","state":"waiting","input":{"kind":"code"}}"#);
        assert!(sparse.input.unwrap().required);
    }

    #[test]
    fn the_address_and_code_stay_out_of_debug_output() {
        let mut sign_in = parse(WAITING);
        sign_in.user_code = Some("ABCD-1234".into());
        let printed = format!("{sign_in:?}");
        assert!(!printed.contains("provider.example") && !printed.contains("ABCD-1234"));
        assert!(printed.contains("anthropic"));
    }

    #[test]
    fn starting_and_finishing_show_progress_and_only_cancel() {
        for (state, status) in [
            ("starting", "Starting the sign-in…"),
            ("finishing", "Finishing the sign-in…"),
        ] {
            // The host can still hold an address while it finishes; it is not offered.
            let mut sign_in = in_state(state);
            sign_in.url = Some("https://provider.example/authorize".into());
            sign_in.user_code = Some("ABCD".into());
            let view = view(&sign_in);
            assert_eq!((view.status.as_str(), view.tone), (status, Tone::Progress));
            assert!(view.poll && view.can_cancel && !view.can_retry);
            assert!(view.destination.is_none() && view.user_code.is_none());
            assert!(view.prompt.is_none() && view.close.is_none() && !view.signed_in);
            assert_eq!(view.remaining.as_deref(), Some("10 min left"));
        }
    }

    #[test]
    fn a_browser_flow_shows_the_address_and_keeps_the_optional_paste_field() {
        let view = view(&parse(WAITING));
        assert_eq!(view.status, "Waiting for you to sign in in your browser");
        assert_eq!(view.tone, Tone::Attention);
        let destination = view.destination.unwrap();
        assert_eq!(destination.host, "provider.example");
        assert_eq!(destination.url, "https://provider.example/authorize?...");
        assert!(destination.encrypted && !view.unusable_address);
        assert_eq!(view.user_code, None);
        assert!(
            view.detail
                .unwrap()
                .starts_with("Complete login in your browser.")
        );
        assert_eq!(view.remaining.as_deref(), Some("9 min 58 s left"));
        assert!(view.poll && view.can_cancel && !view.can_retry && view.close.is_none());
        // Optional, because the browser normally returns by itself, but still offered.
        assert_eq!(
            view.prompt,
            Some(Prompt::Field {
                label: "Complete login in your browser, or paste the authorization code / redirect URL here:".into(),
                placeholder: "http://localhost:53692/callback".into(),
                masked: true,
                optional: true,
                submit: "Submit code",
            })
        );
    }

    #[test]
    fn a_device_flow_shows_the_code_to_type() {
        let mut sign_in = in_state("waiting");
        sign_in.url = Some("https://github.com/login/device".into());
        sign_in.user_code = Some("ABCD-1234".into());
        sign_in.expires_in_seconds = 45;
        let view = view(&sign_in);
        assert_eq!(view.status, "Enter the code on the provider's page");
        assert_eq!(view.user_code.as_deref(), Some("ABCD-1234"));
        assert_eq!(view.destination.unwrap().host, "github.com");
        assert_eq!(view.remaining.as_deref(), Some("45 s left"));
        assert!(view.prompt.is_none() && view.poll && view.can_cancel);
    }

    #[test]
    fn each_input_kind_becomes_the_matching_prompt() {
        let url = Some("https://provider.example/a");
        // A required code: the browser cannot return to this machine.
        let code = view(&asking("code", true, url));
        assert_eq!(
            code.status,
            "Sign in in your browser, then paste the code here"
        );
        assert!(matches!(
            code.prompt,
            Some(Prompt::Field {
                masked: true,
                optional: false,
                submit: "Submit code",
                ..
            })
        ));
        let secret = view(&asking("secret", true, None));
        assert_eq!(secret.status, "The sign-in needs an answer");
        assert!(matches!(
            secret.prompt,
            Some(Prompt::Field {
                masked: true,
                optional: false,
                submit: "Submit",
                ..
            })
        ));
        let text = view(&asking("text", true, None));
        assert!(matches!(
            &text.prompt,
            Some(Prompt::Field { masked: false, optional: false, label, .. }) if label == "Question?"
        ));
        // An input kind from a newer host is still answerable as plain text.
        assert_eq!(view(&asking("passkey", true, None)).prompt, text.prompt);
        let choice = view(&asking("choice", true, None));
        assert_eq!(choice.status, "Choose how to sign in");
        let Some(Prompt::Choice { label, choices }) = choice.prompt else {
            panic!("a choice prompt");
        };
        assert_eq!(label, "Question?");
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>(),
            ["browser", "device"]
        );
        // Without a message, each kind has its own wording.
        let mut bare = asking("code", false, url);
        bare.input.as_mut().unwrap().message.clear();
        assert!(matches!(
            view(&bare).prompt,
            Some(Prompt::Field { label, optional: true, .. }) if label.starts_with("Paste the authorization code")
        ));
        // Waiting with nothing to act on yet.
        assert_eq!(
            view(&in_state("waiting")).status,
            "Waiting for the provider"
        );
    }

    #[test]
    fn final_states_stop_polling_and_say_what_happened() {
        let signed_in = view(&in_state("signed_in"));
        assert_eq!(signed_in.tone, Tone::Success);
        assert!(signed_in.signed_in && !signed_in.poll && !signed_in.can_cancel);
        assert!(!signed_in.can_retry && signed_in.close == Some("Done"));
        assert!(signed_in.remaining.is_none());

        let mut failed = in_state("failed");
        failed.error = Some(Failure {
            code: "permission_denied".into(),
            message: "The provider denied the sign-in.".into(),
        });
        // A stale address must not be offered after the flow ended.
        failed.url = Some("https://provider.example/a".into());
        let failed = view(&failed);
        assert_eq!(
            (failed.status.as_str(), failed.tone),
            ("The sign-in failed", Tone::Failure)
        );
        assert_eq!(
            failed.detail.as_deref(),
            Some("The provider denied the sign-in.")
        );
        assert!(failed.can_retry && !failed.poll && !failed.signed_in);
        assert!(failed.destination.is_none() && failed.close == Some("Close"));
        assert!(
            view(&in_state("failed"))
                .detail
                .unwrap()
                .contains("no reason")
        );

        let cancelled = view(&in_state("cancelled"));
        assert_eq!(cancelled.tone, Tone::Neutral);
        assert!(cancelled.status.contains("cancelled") && cancelled.can_retry && !cancelled.poll);
        let expired = view(&in_state("expired"));
        assert_eq!(expired.tone, Tone::Attention);
        assert!(expired.status.contains("expired") && expired.can_retry && !expired.poll);
    }

    #[test]
    fn an_unfamiliar_state_keeps_polling_and_can_be_cancelled() {
        let view = view(&in_state("verifying_device"));
        assert_eq!(view.tone, Tone::Attention);
        assert!(view.poll && view.can_cancel && view.detail.unwrap().contains("Update Plait"));
        assert!(SignInState::Other.pending() && !SignInState::SignedIn.pending());
        for state in ["failed", "cancelled", "expired", "signed_in"] {
            assert!(!in_state(state).state.pending());
        }
    }

    #[test]
    fn the_destination_is_the_server_the_browser_will_contact() {
        let host = |url: &str| destination(url).map(|destination| destination.host);
        assert_eq!(
            host("https://claude.ai/oauth/authorize?code=1").as_deref(),
            Some("claude.ai")
        );
        assert_eq!(
            host("http://LOCALHOST:1455/start").as_deref(),
            Some("localhost:1455")
        );
        assert!(!destination("http://localhost:1455/").unwrap().encrypted);
        // A user name that looks like a trusted server does not hide the real one.
        assert_eq!(
            host("https://accounts.google.com@evil.example/x").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            host("https://evil.example\\@good.example/").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            host("https://example.com?next=/a").as_deref(),
            Some("example.com")
        );
        for unusable in [
            "",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://",
            "https://:443/",
            "https://exa mple.com/",
            "https://example.com/\npath",
            "ftp://example.com",
        ] {
            assert_eq!(destination(unusable), None, "{unusable:?}");
        }
        let mut sign_in = in_state("waiting");
        sign_in.url = Some("javascript:alert(1)".into());
        let view = view(&sign_in);
        assert!(view.unusable_address && view.destination.is_none());
    }

    #[test]
    fn remaining_time_reads_naturally() {
        assert_eq!(remaining(0), "0 s left");
        assert_eq!(remaining(59), "59 s left");
        assert_eq!(remaining(60), "1 min left");
        assert_eq!(remaining(598), "9 min 58 s left");
    }
}
