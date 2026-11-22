//! Host agent resource wire types.
pub use plait_protocol::host::agent::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_checkout_under_version_3() {
        assert_eq!(
            serde_json::to_value(Request::agent_resources("/repo".into())).unwrap(),
            serde_json::json!({"op": "agent_resources", "checkout": "/repo", "protocol_version": 3})
        );
    }

    #[test]
    fn a_reply_decodes_and_tolerates_later_additions() {
        let reply: AgentResources = serde_json::from_str(
            r#"{"checkout":"/repo","trusted":false,
                "instructions":{"path":"/a/AGENTS.md","bytes":12},
                "skills":[{"name":"notes","description":"Write notes.","origin":"user","path":"/a/skills/notes/SKILL.md","model_invocation":false,"added":1}],
                "templates":[{"name":"review","description":"","argument_hint":null,"origin":"somewhere-new","path":"/a/prompts/review.md"}],
                "problems":[{"path":null,"message":"Only the first 500 of 501 skills are listed."}],
                "later":true}"#,
        )
        .unwrap();
        assert!(reply.mcp_servers.is_empty());
        assert_eq!(reply.instructions.unwrap().bytes, 12);
        assert!(!reply.skills[0].model_invocation);
        assert_eq!(reply.skills[0].origin, Origin::User);
        assert_eq!(reply.templates[0].origin, Origin::Other);
        assert_eq!(reply.problems[0].path, None);
    }
    #[test]
    fn mcp_metadata_and_switch_tolerate_additions_without_a_version_bump() {
        let mut value = serde_json::json!({
            "checkout": "/repo", "trusted": true, "skills": [], "templates": [], "problems": [],
            "mcp_servers": [{"name": "docs", "transport": "http", "source": "/agent/mcp.json",
                "host": "docs.example", "enabled": true, "can_enable": true,
                "needs_sign_in": null, "signed_in": false, "future": 1}]
        });
        let reply: AgentResources = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(reply.mcp_servers[0].transport, Some(McpTransport::Http));
        assert_eq!(reply.mcp_servers[0].needs_sign_in, None);
        assert!(reply.mcp_servers[0].program.is_none());
        assert!(reply.mcp_servers[0].state.is_none());
        assert!(reply.mcp_servers[0].tool_count.is_none());
        value["mcp_servers"][0]["state"] = "connected".into();
        value["mcp_servers"][0]["tool_count"] = 8.into();
        let reply: AgentResources = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(reply.mcp_servers[0].state, Some(McpState::Connected));
        assert_eq!(reply.mcp_servers[0].tool_count, Some(8));
        value["mcp_servers"][0]["argument_count"] = 2.into();
        value["mcp_servers"][0]["definition"] = "a".repeat(64).into();
        value["mcp_servers"][0]["consent"] = "required".into();
        let reply: AgentResources = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(reply.mcp_servers[0].argument_count, Some(2));
        assert_eq!(reply.mcp_servers[0].consent, Some(McpConsent::Required));
        value["mcp_servers"][0]["transport"] = "future".into();
        value["mcp_servers"][0]["state"] = "future".into();
        value["mcp_servers"][0]["consent"] = "future".into();
        let reply: AgentResources = serde_json::from_value(value).unwrap();
        assert_eq!(reply.mcp_servers[0].transport, Some(McpTransport::Other));
        assert_eq!(reply.mcp_servers[0].state, Some(McpState::Other));
        assert_eq!(reply.mcp_servers[0].consent, Some(McpConsent::Other));
        assert_eq!(
            serde_json::to_value(Request::set_mcp_server(
                "/repo".into(),
                "docs".into(),
                false,
                None
            ))
            .unwrap(),
            serde_json::json!({"op": "set_mcp_server", "checkout": "/repo", "name": "docs", "enabled": false, "protocol_version": 3})
        );
        assert_eq!(
            serde_json::to_value(Request::set_mcp_server(
                "/repo".into(),
                "docs".into(),
                true,
                Some("a".repeat(64))
            ))
            .unwrap()["definition"],
            "a".repeat(64)
        );
    }
}
