//! Client side of the host's provider operations: sign-in, API keys, and
//! custom endpoints. docs/HOST.md "Provider sign-in" is the contract.
//!
//! Requests serialize exactly as documented. Replies are read tolerantly: a
//! newer host can add a field or a state without breaking this desktop.

use super::Error;

pub use plait_protocol::host::providers::*;

/// A host without these operations rejects the operation name with
/// `invalid_request`. Use only for a request with nothing else to reject:
/// `list_providers`, or `command_policy` for the open checkout.
pub fn needs_host_update(error: &Error) -> bool {
    matches!(error, Error::Rejected(error) if error.code == "invalid_request")
}

/// What to say about a refused or failed change. Host messages name the field
/// at fault and never repeat its value, so they are shown as sent.
pub fn refusal(error: &Error) -> String {
    let Error::Rejected(rejected) = error else {
        return format!("{error} Nothing was confirmed; check the provider list.");
    };
    let lead = match rejected.code.as_str() {
        "conflict" => "The host refused and changed nothing",
        "invalid_request" => "The host did not accept this",
        "not_found" => "The host no longer has this",
        "busy" => "The host is busy",
        "internal" => "The host could not finish",
        _ => "The host refused",
    };
    format!("{lead}: {}", rejected.message)
}

/// Providers a person is most likely to want, shown before the rest.
const WELL_KNOWN: &[&str] = &[
    "anthropic",
    "openai-codex",
    "openai",
    "openrouter",
    "google",
    "github-copilot",
];

/// Which providers to list and in what order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// Indexes into the provider list.
    pub rows: Vec<usize>,
    /// Providers left out. Search or Show all reaches them.
    pub hidden: usize,
}

/// Rank of a provider in the unsearched list: in use first, then your own
/// servers, then the well-known ones that can be signed in to.
fn rank(provider: &Provider) -> Option<usize> {
    let known = WELL_KNOWN.iter().position(|id| *id == provider.id);
    if provider.is_custom() {
        return Some(if provider.authenticated { 0 } else { 1 });
    }
    if provider.authenticated {
        return Some(0);
    }
    if !provider.connectable() {
        return None;
    }
    let subscription = provider
        .sign_in
        .as_ref()
        .is_some_and(|method| method.subscription);
    match known {
        Some(position) => Some(2 + position),
        None if subscription => Some(2 + WELL_KNOWN.len()),
        None => None,
    }
}

/// Every word of the query must appear in the provider's name or ID.
fn matches(provider: &Provider, words: &[String]) -> bool {
    let name = provider.name.to_lowercase();
    let id = provider.id.to_lowercase();
    words
        .iter()
        .all(|word| name.contains(word) || id.contains(word))
}

/// The providers to show. Without a query: those in use, then the well-known
/// ones, unless `all`. With one: every match, in the same order.
pub fn arrange(providers: &[Provider], query: &str, all: bool) -> Arrangement {
    let words = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let searching = !words.is_empty();
    let mut rows = providers
        .iter()
        .enumerate()
        .filter(|(_, provider)| {
            if searching {
                matches(provider, &words)
            } else {
                all || rank(provider).is_some()
            }
        })
        .map(|(index, provider)| (rank(provider).unwrap_or(usize::MAX), index))
        .collect::<Vec<_>>();
    // Stable: equal ranks keep the host's order.
    rows.sort_by_key(|(rank, _)| *rank);
    let rows = rows.into_iter().map(|(_, index)| index).collect::<Vec<_>>();
    let hidden = if searching {
        0
    } else {
        providers.len() - rows.len()
    };
    Arrangement { rows, hidden }
}

/// Split what a person typed or pasted into model IDs: commas or line breaks
/// separate them, and repeats are dropped.
pub fn parse_model_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for id in text.split([',', '\n', '\r']).map(str::trim) {
        if !id.is_empty() && !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

/// An endpoint ID made from its name: the host accepts 1 to 64 lowercase
/// letters, digits, dots, hyphens, or underscores, starting with a letter or digit.
pub fn endpoint_id(name: &str) -> String {
    let mut id = String::new();
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            id.push(ch);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
        if id.len() == 64 {
            break;
        }
    }
    id.trim_end_matches('-').to_owned()
}

/// What an endpoint form holds, before it becomes a request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointDraft {
    pub name: String,
    pub base_url: String,
    pub model_ids: Vec<String>,
}

impl EndpointDraft {
    /// The first problem the host would reject, in the form's own words.
    /// The host checks again; this only saves a round trip.
    pub fn problem(&self) -> Option<&'static str> {
        let name = self.name.trim();
        let url = self.base_url.trim();
        if name.is_empty() {
            Some("Enter a name for the server.")
        } else if name.len() > 128 {
            Some("The name is longer than 128 bytes.")
        } else if endpoint_id(name).is_empty() {
            Some("The name needs at least one letter or digit.")
        } else if let Some(problem) = url_problem(url) {
            Some(problem)
        } else if self.model_ids.is_empty() {
            Some("Add at least one model ID, or check the connection and pick from its list.")
        } else if self.model_ids.len() > MODEL_ID_LIMIT {
            Some("A server can list at most 200 model IDs.")
        } else if self.model_ids.iter().any(|id| id.chars().count() > 256) {
            Some("A model ID is longer than 256 characters.")
        } else {
            None
        }
    }
}

/// Why a base URL cannot be sent, if it cannot.
pub fn url_problem(url: &str) -> Option<&'static str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    let Some(rest) = rest else {
        return Some("Enter a base URL that starts with http:// or https://.");
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        Some("The base URL has no server name.")
    } else if authority.contains('@') {
        Some("Put the key in the key field, not in the base URL.")
    } else if url.contains(['?', '#']) {
        Some("The base URL cannot have a query or fragment.")
    } else if url.len() > 2048 {
        Some("The base URL is longer than 2048 bytes.")
    } else if url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        Some("The base URL cannot contain spaces.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostError;

    fn json(request: &Request) -> String {
        serde_json::to_string(request).unwrap()
    }

    fn secret(text: &str) -> Secret {
        Secret::new(text.into())
    }

    #[test]
    fn requests_serialize_as_documented() {
        let cases = [
            (Request::ReloadProviders, r#"{"op":"reload_providers"}"#),
            (Request::ListProviders, r#"{"op":"list_providers"}"#),
            (
                Request::StartSignIn {
                    provider: "anthropic".into(),
                    method: Some("browser".into()),
                },
                r#"{"op":"start_sign_in","provider":"anthropic","method":"browser"}"#,
            ),
            (
                Request::StartSignIn {
                    provider: "anthropic".into(),
                    method: None,
                },
                r#"{"op":"start_sign_in","provider":"anthropic"}"#,
            ),
            (
                Request::StartHarnessSignIn {
                    harness: crate::host::harness::HarnessId::Claude,
                },
                r#"{"op":"start_harness_sign_in","harness":"claude"}"#,
            ),
            (
                Request::SignInStatus {
                    sign_in_id: "HOST_UUID".into(),
                },
                r#"{"op":"sign_in_status","sign_in_id":"HOST_UUID"}"#,
            ),
            (
                Request::AnswerSignIn {
                    sign_in_id: "HOST_UUID".into(),
                    input: secret("Pasted code or choice ID"),
                },
                r#"{"op":"answer_sign_in","sign_in_id":"HOST_UUID","input":"Pasted code or choice ID"}"#,
            ),
            (
                Request::CancelSignIn {
                    sign_in_id: "HOST_UUID".into(),
                },
                r#"{"op":"cancel_sign_in","sign_in_id":"HOST_UUID"}"#,
            ),
            (
                Request::SetProviderKey {
                    provider: "openrouter".into(),
                    key: secret("SECRET"),
                },
                r#"{"op":"set_provider_key","provider":"openrouter","key":"SECRET"}"#,
            ),
            (
                Request::SignOutProvider {
                    provider: "openrouter".into(),
                },
                r#"{"op":"sign_out_provider","provider":"openrouter"}"#,
            ),
            (
                Request::AddEndpoint {
                    id: "home".into(),
                    name: "Home server".into(),
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    model_ids: vec!["MODEL".into()],
                    key: Some(secret("SECRET")),
                },
                r#"{"op":"add_endpoint","id":"home","name":"Home server","base_url":"http://127.0.0.1:8080/v1","model_ids":["MODEL"],"key":"SECRET"}"#,
            ),
            (
                Request::AddEndpoint {
                    id: "home".into(),
                    name: "Home server".into(),
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    model_ids: vec!["MODEL".into()],
                    key: None,
                },
                r#"{"op":"add_endpoint","id":"home","name":"Home server","base_url":"http://127.0.0.1:8080/v1","model_ids":["MODEL"]}"#,
            ),
            (
                Request::UpdateEndpoint {
                    id: "home".into(),
                    name: Some("Home server".into()),
                    base_url: Some("http://127.0.0.1:8080/v1".into()),
                    model_ids: Some(vec!["MODEL".into()]),
                    key: Some(secret("SECRET")),
                },
                r#"{"op":"update_endpoint","id":"home","name":"Home server","base_url":"http://127.0.0.1:8080/v1","model_ids":["MODEL"],"key":"SECRET"}"#,
            ),
            (
                // Only `id` is required; a field left out keeps its saved value.
                Request::UpdateEndpoint {
                    id: "home".into(),
                    name: None,
                    base_url: None,
                    model_ids: None,
                    key: None,
                },
                r#"{"op":"update_endpoint","id":"home"}"#,
            ),
            (
                Request::RemoveEndpoint { id: "home".into() },
                r#"{"op":"remove_endpoint","id":"home"}"#,
            ),
            (
                Request::CheckAddress {
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    key: Some(secret("SECRET")),
                },
                r#"{"op":"check_endpoint","base_url":"http://127.0.0.1:8080/v1","key":"SECRET"}"#,
            ),
            (
                Request::CheckAddress {
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    key: None,
                },
                r#"{"op":"check_endpoint","base_url":"http://127.0.0.1:8080/v1"}"#,
            ),
            (
                Request::CheckSaved { id: "home".into() },
                r#"{"op":"check_endpoint","id":"home"}"#,
            ),
        ];
        for (request, documented) in cases {
            assert_eq!(json(&request), documented);
        }
    }

    const ANTHROPIC: &str = r#"{"id":"anthropic","name":"Anthropic","kind":"builtin","authenticated":false,"credential":"none","model_count":18,"sign_in":{"name":"Anthropic (Claude Pro/Max)","subscription":true},"api_key":{"name":"Anthropic API key"},"endpoint":null}"#;
    const HOME: &str = r#"{"id":"home","name":"Home server","kind":"custom","authenticated":true,"credential":"external","model_count":1,"sign_in":null,"api_key":{"name":"API key"},"endpoint":{"base_url":"http://127.0.0.1:8080/v1","api":"openai-completions","model_ids":["MODEL"]}}"#;

    fn provider(text: &str) -> Provider {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn provider_replies_read_the_documented_json() {
        let anthropic = provider(ANTHROPIC);
        assert_eq!(anthropic.id, "anthropic");
        assert_eq!(anthropic.kind, ProviderKind::Builtin);
        assert_eq!(anthropic.credential, Credential::None);
        assert_eq!(anthropic.model_count, 18);
        assert!(anthropic.sign_in.as_ref().unwrap().subscription);
        assert_eq!(
            anthropic.api_key.as_ref().unwrap().name,
            "Anthropic API key"
        );
        assert!(anthropic.endpoint.is_none() && !anthropic.authenticated);

        let home = provider(HOME);
        assert_eq!(home.kind, ProviderKind::Custom);
        assert_eq!(home.credential, Credential::External);
        let endpoint = home.endpoint.as_ref().unwrap();
        assert_eq!(
            endpoint.base_url.as_deref(),
            Some("http://127.0.0.1:8080/v1")
        );
        assert_eq!(endpoint.api.as_deref(), Some("openai-completions"));
        assert_eq!(endpoint.model_ids, ["MODEL"]);

        let list: ProvidersReply =
            serde_json::from_str(&format!(r#"{{"providers":[{ANTHROPIC},{HOME}]}}"#)).unwrap();
        assert_eq!(list.providers, [anthropic.clone(), home.clone()]);
        let one: ProviderReply =
            serde_json::from_str(&format!(r#"{{"provider":{HOME}}}"#)).unwrap();
        assert_eq!(one.provider, home);
        let removed: RemovedReply = serde_json::from_str(r#"{"removed":"home"}"#).unwrap();
        assert_eq!(removed.removed, "home");
    }

    #[test]
    fn replies_tolerate_unknown_fields_values_and_key_order() {
        // A newer host: an added field at each level, and values this desktop does not know.
        let newer = r#"{"added":{"x":1},"api_key":null,"authenticated":true,"credential":"keychain","endpoint":{"api":"responses","base_url":null,"headers":2,"model_ids":[]},"id":"p","kind":"plugin","model_count":2,"name":"P","sign_in":{"name":"P account","plan":"team"}}"#;
        let provider = provider(newer);
        assert_eq!(provider.kind, ProviderKind::Other);
        assert_eq!(provider.credential, Credential::Other);
        assert_eq!(provider.credential_label(), "Signed in");
        assert!(!provider.sign_in.as_ref().unwrap().subscription);
        assert_eq!(provider.endpoint.as_ref().unwrap().base_url, None);
        // An unknown credential offers no Sign out; the sign-in it names is still offered.
        assert_eq!(provider.actions(), [ProviderAction::SignIn]);
        // A sparse entry still loads, with nothing offered.
        let sparse: Provider = serde_json::from_str(r#"{"id":"q","name":"Q"}"#).unwrap();
        assert!(sparse.actions().is_empty() && !sparse.connectable());
        let reply: ProvidersReply =
            serde_json::from_str(r#"{"next_page":null,"providers":[]}"#).unwrap();
        assert!(reply.providers.is_empty());
        assert_eq!(reply.warning_text(), None);
    }

    #[test]
    fn a_reload_warning_is_explained_and_an_unknown_one_is_not_shown() {
        let stale: ProvidersReply =
            serde_json::from_str(r#"{"providers":[],"warning":"login_environment_unavailable"}"#)
                .unwrap();
        assert_eq!(
            stale.warning_text(),
            Some("Couldn't read your shell's environment; keys from it may be out of date.")
        );
        let newer: ProvidersReply =
            serde_json::from_str(r#"{"providers":[],"warning":"something_new"}"#).unwrap();
        assert_eq!(newer.warning_text(), None);
    }

    #[test]
    fn endpoint_checks_read_the_documented_json_and_explain_themselves() {
        let ok: EndpointCheck = serde_json::from_str(
            r#"{"reachable":true,"reason":null,"status":200,"model_ids":["MODEL"],"truncated":false}"#,
        )
        .unwrap();
        assert!(ok.reachable && ok.reason.is_none() && !ok.truncated);
        assert_eq!(
            (ok.status, ok.model_ids.as_slice()),
            (Some(200), &["MODEL".to_owned()][..])
        );
        assert_eq!(ok.summary(), "The server answered and reports 1 model.");

        let refused: EndpointCheck = serde_json::from_str(
            r#"{"reachable":false,"reason":"unauthorized","status":401,"model_ids":[],"truncated":false}"#,
        )
        .unwrap();
        assert_eq!(refused.reason, Some(CheckReason::Unauthorized));
        assert_eq!(refused.summary(), "The server refused the key. (HTTP 401)");

        for (reason, expected) in [
            ("timeout", CheckReason::Timeout),
            ("unreachable", CheckReason::Unreachable),
            ("not_found", CheckReason::NotFound),
            ("http_error", CheckReason::HttpError),
            ("invalid_response", CheckReason::InvalidResponse),
            ("something_new", CheckReason::Other),
        ] {
            let check: EndpointCheck = serde_json::from_str(&format!(
                r#"{{"model_ids":[],"reachable":false,"reason":"{reason}","status":null,"truncated":false,"took_ms":12}}"#
            ))
            .unwrap();
            assert_eq!(check.reason, Some(expected));
            assert!(check.status.is_none() && !check.summary().is_empty());
        }
        let many = EndpointCheck {
            reachable: true,
            reason: None,
            status: Some(200),
            model_ids: vec!["a".into(), "b".into()],
            truncated: true,
        };
        assert_eq!(
            many.summary(),
            "The server answered and reports 2 models. The first 500 are listed."
        );
    }

    fn rejected(code: &str, message: &str) -> Error {
        Error::Rejected(HostError {
            retry_after_seconds: None,
            code: code.into(),
            message: message.into(),
        })
    }

    /// A private socket whose host answers one request with `reply`. Returns
    /// the client, a handle that yields the request line, and the directory.
    fn fake_host(
        name: &str,
        reply: &'static str,
    ) -> (
        crate::host::Client,
        std::thread::JoinHandle<String>,
        std::path::PathBuf,
    ) {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let directory =
            std::env::temp_dir().join(format!("plait-providers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            socket.write_all(reply.as_bytes()).unwrap();
            socket.write_all(b"\n").unwrap();
            line
        });
        (
            crate::host::Client {
                transport: crate::host::Transport::Local(path),
            },
            server,
            directory,
        )
    }

    #[test]
    fn an_unknown_operation_means_the_host_needs_an_update() {
        // What a host without the operation answers, over a real socket.
        let (client, server, directory) = fake_host(
            "old",
            r#"{"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}"#,
        );
        let error = client
            .send::<_, ProvidersReply>(&Request::ListProviders)
            .unwrap_err();
        assert_eq!(server.join().unwrap(), "{\"op\":\"list_providers\"}\n");
        std::fs::remove_dir_all(directory).unwrap();
        assert!(needs_host_update(&error));
        assert!(!needs_host_update(&rejected("internal", "Reload failed.")));
        assert!(!needs_host_update(&rejected(
            "not_found",
            "Unknown provider."
        )));
        assert!(!needs_host_update(&Error::Transport("no socket".into())));
        assert!(!needs_host_update(&Error::Protocol("bad reply".into())));
    }

    #[test]
    fn a_reply_with_reordered_keys_and_added_fields_is_read_from_the_socket() {
        let (client, server, directory) = fake_host(
            "new",
            r#"{"ok":true,"result":{"provider":{"api_key":{"name":"OpenRouter API key"},"authenticated":true,"credential":"api_key","endpoint":null,"id":"openrouter","kind":"builtin","model_count":214,"name":"OpenRouter","refreshed_at":1,"sign_in":{"name":"OpenRouter OAuth","subscription":false}},"reloaded":true}}"#,
        );
        let reply: ProviderReply = client
            .send(&Request::SetProviderKey {
                provider: "openrouter".into(),
                key: secret("SECRET"),
            })
            .unwrap();
        assert_eq!(
            server.join().unwrap(),
            "{\"op\":\"set_provider_key\",\"provider\":\"openrouter\",\"key\":\"SECRET\"}\n"
        );
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(reply.provider.credential, Credential::ApiKey);
        assert_eq!(reply.provider.model_count, 214);
        assert_eq!(reply.provider.credential_label(), "API key saved");
    }

    #[test]
    fn refusals_are_presented_with_the_hosts_reason() {
        let conflict = rejected(
            "conflict",
            "models.json has comments; a rewrite would delete them.",
        );
        assert_eq!(
            refusal(&conflict),
            "The host refused and changed nothing: models.json has comments; a rewrite would delete them."
        );
        assert!(
            refusal(&rejected("busy", "Four sign-ins are in progress."))
                .starts_with("The host is busy")
        );
        assert!(
            refusal(&rejected("invalid_request", "key is too long.")).contains("key is too long.")
        );
        assert!(
            refusal(&Error::Transport("Host disconnected.".into()))
                .contains("Nothing was confirmed")
        );
    }

    const KEY: &str = "sk-sample-0123456789-not-a-real-key";

    #[test]
    fn a_secret_is_not_printed_and_is_overwritten() {
        let request = Request::SetProviderKey {
            provider: "openrouter".into(),
            key: secret(KEY),
        };
        assert!(!format!("{request:?}").contains(KEY));
        assert!(format!("{request:?}").contains("Secret(…)"));
        let request = Request::AddEndpoint {
            id: "home".into(),
            name: "Home".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            model_ids: vec!["m".into()],
            key: Some(secret(KEY)),
        };
        assert!(!format!("{request:?}").contains(KEY));
        let mut text = KEY.to_owned();
        wipe(&mut text);
        assert!(text.is_empty());
        assert!(secret("  ").is_empty() && !secret(KEY).is_empty());
        assert_eq!(secret(KEY).len(), KEY.len());
    }

    #[test]
    fn a_failed_send_never_repeats_the_secret() {
        // No host: the socket path does not exist.
        let client = crate::host::Client {
            transport: crate::host::Transport::Local(
                std::env::temp_dir().join("plait-no-such-host/host.sock"),
            ),
        };
        let request = Request::SetProviderKey {
            provider: "openrouter".into(),
            key: secret(KEY),
        };
        let error = client.send::<_, ProviderReply>(&request).unwrap_err();
        assert!(matches!(error, Error::Transport(_)));
        assert!(!error.to_string().contains(KEY) && !refusal(&error).contains(KEY));
        // Too large to send: refused before any connection, without the text.
        let huge = Request::AnswerSignIn {
            sign_in_id: "id".into(),
            input: Secret::new(KEY.repeat(4096)),
        };
        let error = client.send::<_, ProviderReply>(&huge).unwrap_err();
        assert!(matches!(&error, Error::Protocol(message) if message.contains("64 KiB")));
        assert!(!error.to_string().contains(KEY));
        // A host's refusal names the field, never the value.
        let refused = rejected(
            "invalid_request",
            "key must not contain control characters.",
        );
        assert!(!refusal(&refused).contains(KEY));
    }

    fn sample(id: &str, authenticated: bool, sign_in: Option<bool>, key: bool) -> Provider {
        Provider {
            id: id.into(),
            name: id.to_uppercase(),
            kind: ProviderKind::Builtin,
            authenticated,
            credential: if authenticated {
                Credential::ApiKey
            } else {
                Credential::None
            },
            model_count: 3,
            sign_in: sign_in.map(|subscription| SignInMethod {
                name: format!("{id} sign-in"),
                subscription,
            }),
            api_key: key.then(|| KeyMethod {
                name: format!("{id} key"),
            }),
            endpoint: None,
        }
    }

    fn catalog() -> Vec<Provider> {
        let mut home = sample("home", true, None, true);
        home.kind = ProviderKind::Custom;
        home.credential = Credential::External;
        vec![
            sample("groq", false, None, true),
            sample("openrouter", false, Some(false), true),
            sample("anthropic", false, Some(true), true),
            sample("zed-plan", false, Some(true), false),
            sample("mistral", true, None, true),
            sample("bedrock", false, None, false),
            home,
            sample("openai-codex", false, Some(true), false),
        ]
    }

    fn ids(providers: &[Provider], arrangement: &Arrangement) -> Vec<String> {
        arrangement
            .rows
            .iter()
            .map(|index| providers[*index].id.clone())
            .collect()
    }

    #[test]
    fn providers_in_use_come_first_then_the_well_known_ones() {
        let providers = catalog();
        let shown = arrange(&providers, "", false);
        assert_eq!(
            ids(&providers, &shown),
            [
                "mistral",
                "home",
                "anthropic",
                "openai-codex",
                "openrouter",
                "zed-plan"
            ]
        );
        // Groq takes a key but is not well known; Bedrock cannot be signed in to here.
        assert_eq!(shown.hidden, 2);
        let all = arrange(&providers, "", true);
        assert_eq!(all.rows.len(), providers.len());
        assert_eq!(all.hidden, 0);
        assert_eq!(ids(&providers, &all)[..2], ["mistral", "home"]);
    }

    #[test]
    fn search_reaches_the_whole_catalog_by_name_or_id() {
        let providers = catalog();
        assert_eq!(
            ids(&providers, &arrange(&providers, "GRO", false)),
            ["groq"]
        );
        assert_eq!(
            ids(&providers, &arrange(&providers, "open", false)),
            ["openai-codex", "openrouter"]
        );
        assert_eq!(
            ids(&providers, &arrange(&providers, "open codex", false)),
            ["openai-codex"]
        );
        let none = arrange(&providers, "zzzz", false);
        assert!(none.rows.is_empty() && none.hidden == 0);
    }

    #[test]
    fn a_row_offers_only_the_actions_that_apply() {
        use ProviderAction::*;
        let providers = catalog();
        let by_id = |id: &str| providers.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("anthropic").actions(), [SignIn, EnterKey]);
        assert_eq!(by_id("openai-codex").actions(), [SignIn]);
        assert_eq!(by_id("groq").actions(), [EnterKey]);
        assert_eq!(by_id("mistral").actions(), [EnterKey, SignOut]);
        assert!(by_id("bedrock").actions().is_empty());
        assert_eq!(by_id("home").actions(), [Check, Edit, Remove]);
        let mut signed_in = by_id("anthropic").clone();
        signed_in.authenticated = true;
        signed_in.credential = Credential::SignIn;
        assert_eq!(signed_in.actions(), [EnterKey, SignOut]);
        assert_eq!(
            signed_in.credential_label(),
            "Signed in with a subscription"
        );
        // An external credential is not the host's to remove.
        let mut external = by_id("groq").clone();
        external.authenticated = true;
        external.credential = Credential::External;
        assert_eq!(external.actions(), [EnterKey]);
        assert_eq!(external.credential_label(), "Configured outside Plait");
    }

    #[test]
    fn authentication_is_described_in_words() {
        let providers = catalog();
        let by_id = |id: &str| providers.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("anthropic").credential_label(), "Not signed in");
        assert_eq!(by_id("mistral").credential_label(), "API key saved");
        assert_eq!(
            by_id("home").credential_label(),
            "Usable; no key saved in Plait"
        );
        let mut account = by_id("openrouter").clone();
        account.credential = Credential::SignIn;
        assert_eq!(account.credential_label(), "Signed in with an account");
        assert_eq!(by_id("mistral").model_label(), "3 models");
        let mut one = by_id("home").clone();
        one.model_count = 1;
        assert_eq!(one.model_label(), "1 model");
    }

    #[test]
    fn model_ids_and_endpoint_ids_come_from_what_was_typed() {
        assert_eq!(
            parse_model_ids(" qwen3:32b, llama-4\nqwen3:32b,,\r\n mistral/large "),
            ["qwen3:32b", "llama-4", "mistral/large"]
        );
        assert!(parse_model_ids(" , \n").is_empty());
        assert_eq!(endpoint_id("Home server"), "home-server");
        assert_eq!(endpoint_id("  LM Studio (desk) "), "lm-studio-desk");
        assert_eq!(endpoint_id("ünïcode 2"), "n-code-2");
        assert_eq!(endpoint_id("---"), "");
        assert_eq!(endpoint_id(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn an_endpoint_draft_reports_its_first_problem() {
        let mut draft = EndpointDraft::default();
        assert_eq!(draft.problem(), Some("Enter a name for the server."));
        draft.name = "Home".into();
        assert!(draft.problem().unwrap().contains("http://"));
        draft.base_url = "http://user:pw@host/v1".into();
        assert!(draft.problem().unwrap().contains("key field"));
        draft.base_url = "https://host/v1?x=1".into();
        assert!(draft.problem().unwrap().contains("query"));
        draft.base_url = "http://".into();
        assert!(draft.problem().unwrap().contains("server name"));
        draft.base_url = "http://127.0.0.1:8080/v1".into();
        assert!(draft.problem().unwrap().contains("model ID"));
        draft.model_ids = vec!["m".into()];
        assert_eq!(draft.problem(), None);
        draft.model_ids = (0..201).map(|index| index.to_string()).collect();
        assert!(draft.problem().unwrap().contains("200"));
        assert_eq!(url_problem("ftp://host"), url_problem("host"));
        assert!(url_problem("http://host /v1").is_some());
    }
}
