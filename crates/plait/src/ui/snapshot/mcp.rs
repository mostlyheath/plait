//! MCP settings use injected metadata; these scenes never contact a host or server.
use super::{Harness, fixture_repository, isolated};
use crate::ui::settings_agent::AgentView;
use plait_client::host::agent::{AgentResources, McpConsent, McpServer, McpState, McpTransport};
use std::{path::Path, sync::Arc};

fn resources(checkout: &Path) -> AgentResources {
    let server = |name: &str, transport, enabled, needs_sign_in, signed_in| McpServer {
        name: name.into(),
        transport: Some(transport),
        source: "/opt/sample-agent/mcp.json".into(),
        override_source: None,
        program: (transport == McpTransport::Stdio).then(|| "uvx".into()),
        argument_count: (transport == McpTransport::Stdio).then_some(2),
        host: (transport == McpTransport::Http).then(|| "docs.example".into()),
        enabled,
        can_enable: true,
        needs_sign_in,
        signed_in,
        disabled_reason: (!enabled).then(|| "Off until enabled for this sandboxed project.".into()),
        problem: None,
        state: None,
        tool_count: None,
        definition: Some("a".repeat(64)),
        consent: None,
    };
    let mut invalid = server("invalid-config", McpTransport::Http, false, None, false);
    invalid.transport = None;
    invalid.host = None;
    invalid.can_enable = false;
    invalid.problem = Some("url must be an http or https URL".into());
    invalid.disabled_reason = invalid.problem.clone();
    AgentResources {
        checkout: checkout.into(),
        trusted: false,
        instructions: None,
        skills: vec![],
        templates: vec![],
        problems: vec![],
        mcp_servers: vec![
            server("local-docs", McpTransport::Stdio, true, Some(false), false),
            server(
                "project-search-with-a-long-name-that-still-leaves-room-for-the-switch",
                McpTransport::Http,
                false,
                None,
                false,
            ),
            server("design", McpTransport::Http, true, Some(true), false),
            server("issues", McpTransport::Http, true, Some(true), true),
            invalid,
        ],
    }
}

fn review_resources(checkout: &Path) -> AgentResources {
    let mut reply = resources(checkout);
    reply.trusted = true;
    let project = checkout.join(".pi/mcp.json");
    let server = &mut reply.mcp_servers[0];
    server.source = project.clone();
    server.enabled = false;
    server.consent = Some(McpConsent::Required);
    server.disabled_reason =
        Some("Defined by this project. Read its configuration file before turning it on.".into());
    let server = &mut reply.mcp_servers[1];
    server.source = project.clone();
    server.consent = Some(McpConsent::Changed);
    server.disabled_reason = Some(
        "The server definition changed. Read its configuration file and turn it on again.".into(),
    );
    let server = &mut reply.mcp_servers[2];
    server.name = "project-tool-limits".into();
    server.needs_sign_in = Some(false);
    server.override_source = Some(project);
    server.consent = Some(McpConsent::Restricted);
    server.disabled_reason = Some("Project overrides are limited by your user configuration until you explicitly turn this server on here.".into());
    reply
}

pub(super) fn mcp_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        plait.show_project_settings(window, cx);
        plait.policy.key = Some((
            plait.host.connection.epoch,
            plait.host.creation.checkout.clone(),
        ));
        plait.policy.value = Some(plait_client::host::policy::CommandPolicy::Sandboxed);
        plait.agent.key = Some((
            plait.host.connection.epoch,
            plait.host.creation.checkout.clone(),
        ));
        plait.agent.view = Some(Arc::new(AgentView::new(resources(
            &plait.host.creation.checkout,
        ))));
        plait
            .settings_scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(-180.)));
    });
    harness.capture(&format!("project-settings-mcp-{suffix}"));
    harness.update(|plait, _, _| {
        let mut reply = resources(&plait.host.creation.checkout);
        reply.mcp_servers[0].state = Some(McpState::Connected);
        reply.mcp_servers[0].tool_count = Some(8);
        reply.mcp_servers[1].state = Some(McpState::Stopped);
        reply.mcp_servers[1].tool_count = Some(3);
        plait.agent.view = Some(Arc::new(AgentView::new(reply)));
    });
    harness.capture(&format!("project-settings-mcp-observed-{suffix}"));
    harness.update(|plait, _, _| {
        plait.policy.value = Some(plait_client::host::policy::CommandPolicy::Trusted);
        plait.agent.view = Some(Arc::new(AgentView::new(review_resources(
            &plait.host.creation.checkout,
        ))));
    });
    harness.capture(&format!("project-settings-mcp-review-{suffix}"));
    harness.update(|plait, _, _| {
        plait.policy.value = Some(plait_client::host::policy::CommandPolicy::Sandboxed);
        let mut reply = resources(&plait.host.creation.checkout);
        reply.mcp_servers.clear();
        plait.agent.view = Some(Arc::new(AgentView::new(reply)));
        plait
            .settings_scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
    });
    harness.capture(&format!("project-settings-no-mcp-{suffix}"));
    harness.update(|plait, _, _| plait.settings_open = false);
    harness.connect(repository, true);
    harness.change_snapshot(|snapshot| {
        snapshot.messages[0].text = "Find the setup guide in our configured documentation server.".into();
        snapshot.messages[1].text = "I found the documentation search tool. I’ll use it for the setup guide.".into();
        snapshot.tools[0].name = "tool_search".into();
        snapshot.tools[0].input = r#"{"query":"documentation search"}"#.into();
        snapshot.tools[0].output = "Loaded the documentation search tool.".into();
        snapshot.tools[1].name = "mcp:docs/search".into();
        snapshot.tools[1].input = r#"{"query":"setup guide"}"#.into();
        let approval = snapshot.pending_approval.as_mut().unwrap();
        approval.summary = "Use search from docs".into();
        approval.details = r#"{"query":"setup guide"}"#.into();
        approval.dirty_paths.clear();
        approval.risk = "Only this exact MCP call is approved. The server can use the network. Local servers can read the checkout and command toolchain paths; writes are limited to private scratch and the thread build cache. Server hints never waive approval.".into();
    });
    harness.capture(&format!("mcp-tool-approval-{suffix}"));
    harness.change_snapshot(|snapshot| {
        let input = serde_json::json!({"query": "safe\u{202e}delete\u{200b}\u{0085}"}).to_string();
        snapshot.tools[1].input = input.clone();
        snapshot.tools[1].output = "Result: safe\u{202e}delete\u{200b}\u{2028}".into();
        snapshot.pending_approval.as_mut().unwrap().details =
            plait_client::display_text::escape(&input);
    });
    harness.update(|plait, _, _| {
        let tool = plait.host.thread.snapshot.as_ref().unwrap().tools[1]
            .id
            .clone();
        plait.workspace.expanded_live_tools.insert(tool);
    });
    harness.capture(&format!("mcp-tool-controls-{suffix}"));
}

#[test]
fn mcp_rows_explain_configuration_and_sign_in() {
    let view = AgentView::new(resources(Path::new("/repo")));
    let rows = &view.mcp_servers;
    assert_eq!(rows[0].state, "Ready to search");
    assert!(rows[0].enabled && rows[0].hover.contains("uvx"));
    assert!(
        rows[0]
            .hover
            .contains("2 arguments; their values are hidden")
    );
    assert!(rows[0].hover.contains("before any call is approved"));
    assert!(rows[0].file.is_none());
    assert_eq!(rows[1].state, "Off");
    assert!(rows[1].hover.contains("sandboxed project"));
    assert_eq!(rows[2].state, "Sign in with Pi");
    assert_eq!(rows[3].state, "Sign-in saved");
    assert!(rows[3].hover.contains("validity has not been checked"));
    assert_eq!(rows[4].state, "Invalid");
    let mut old = resources(Path::new("/repo"));
    old.mcp_servers.clear();
    assert!(AgentView::new(old).mcp_servers.is_empty());
    let mut observed = resources(Path::new("/repo"));
    observed.mcp_servers[0].state = Some(McpState::Connected);
    observed.mcp_servers[0].tool_count = Some(8);
    assert_eq!(
        AgentView::new(observed).mcp_servers[0].state,
        "Connected · 8 tools"
    );
    let review = AgentView::new(review_resources(Path::new("/repo")));
    assert_eq!(review.mcp_servers[0].state, "Needs review");
    assert_eq!(review.mcp_servers[1].state, "Changed");
    assert_eq!(review.mcp_servers[2].state, "User limits");
    for row in &review.mcp_servers[..3] {
        assert_eq!(row.file.as_deref(), Some(Path::new(".pi/mcp.json")));
    }
}

#[test]
fn mcp_replies_are_guarded_by_checkout_connection_and_refresh() {
    if !isolated("mcp::mcp_replies_are_guarded_by_checkout_connection_and_refresh") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-mcp-view-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    std::fs::create_dir_all(repository.join(".pi")).unwrap();
    std::fs::write(repository.join(".pi/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.update(|plait, _, cx| {
        let key = (
            plait.host.connection.epoch,
            plait.host.creation.checkout.clone(),
        );
        plait.agent.key = Some(key.clone());
        let revision = plait.agent.revision;
        let reply = || Ok(AgentView::new(resources(&key.1)));
        assert!(plait.accept_agent(&key, revision, reply()));
        assert_eq!(plait.agent.view.as_ref().unwrap().mcp_servers.len(), 5);
        assert!(!plait.accept_agent(&(key.0 + 1, key.1.clone()), revision, reply()));
        assert!(!plait.accept_agent(&(key.0, "/another".into()), revision, reply()));
        plait.forget_agent();
        assert!(!plait.accept_agent(&key, revision, reply()));
        // A policy refresh can reopen the same checkout on the same host connection.
        plait.agent.key = Some(key.clone());
        assert!(!plait.accept_agent(&key, revision, reply()));
        assert!(plait.accept_agent(&key, plait.agent.revision, reply()));
        assert_eq!(
            plait.agent.view.as_ref().unwrap().mcp_servers[0]
                .name
                .as_ref(),
            "local-docs"
        );
        let review = AgentView::new(review_resources(&repository));
        plait.settings_open = true;
        plait.open_file(review.mcp_servers[0].file.clone().unwrap(), None, cx);
        assert!(!plait.settings_open);
        assert_eq!(
            plait.workspace.file_viewer.path.as_deref(),
            Some(Path::new(".pi/mcp.json"))
        );
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}
