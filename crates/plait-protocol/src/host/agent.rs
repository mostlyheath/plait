//! Client side of `agent_resources`: what the person's Pi setup offers runs in
//! one checkout. docs/HOST.md "Skills and user instructions" is the contract.
//!
//! The reply names files and never holds their contents. New fields may be
//! added, so unknown ones are ignored; an unknown source reads as `Other`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Hosts from before this operation reject it as `invalid_request`.
    SetMcpServer {
        checkout: PathBuf,
        name: String,
        enabled: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        definition: Option<String>,
        protocol_version: u32,
    },
    AgentResources {
        checkout: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<super::git::WireIdentity>,
        protocol_version: u32,
    },
}

impl Request {
    pub fn set_mcp_server(
        checkout: PathBuf,
        name: String,
        enabled: bool,
        definition: Option<String>,
    ) -> Self {
        Self::SetMcpServer {
            checkout,
            name,
            enabled,
            definition,
            protocol_version: 3,
        }
    }

    pub fn agent_resources(checkout: PathBuf) -> Self {
        Self::AgentResources {
            checkout,
            identity: None,
            protocol_version: 3,
        }
    }
}

/// Where a skill or template came from, as Pi resolved it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The Pi agent folder or `~/.agents`.
    User,
    /// The checkout, offered only when the project is trusted.
    Project,
    /// An installed Pi package.
    Package,
    /// A path named in Pi's settings.
    Path,
    #[serde(other)]
    Other,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "Your agent",
            Self::Project => "Project",
            Self::Package => "Package",
            Self::Path => "Settings path",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Instructions {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub origin: Origin,
    pub path: PathBuf,
    /// False for a skill only `/skill:name` loads; the model is not offered it.
    pub model_invocation: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub origin: Origin,
    pub path: PathBuf,
}

/// Something Pi or Plait could not use, with the file it concerns when there is one.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Problem {
    pub path: Option<PathBuf>,
    pub message: String,
}

/// A configured MCP transport. Invalid entries need not identify a transport.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpState {
    Connected,
    Stopped,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpConsent {
    Required,
    Changed,
    Restricted,
    #[serde(other)]
    Other,
}

/// Metadata only: no arguments, environment, headers, URL paths, or credentials.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub transport: Option<McpTransport>,
    pub source: PathBuf,
    pub override_source: Option<PathBuf>,
    pub program: Option<String>,
    pub argument_count: Option<u32>,
    pub host: Option<String>,
    pub enabled: bool,
    pub can_enable: bool,
    /// None means a connection would be needed to know whether sign-in is required.
    pub needs_sign_in: Option<bool>,
    /// Pi holds tokens; this does not claim that they are still valid.
    pub signed_in: bool,
    pub disabled_reason: Option<String>,
    pub problem: Option<String>,
    /// Observed in this host's lifetime, absent until a run starts the server.
    pub state: Option<McpState>,
    pub tool_count: Option<u32>,
    pub definition: Option<String>,
    pub consent: Option<McpConsent>,
}

/// What a run in `checkout` would be offered now, under its current policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AgentResources {
    pub checkout: PathBuf,
    /// Project skills and templates load only for a trusted project.
    pub trusted: bool,
    pub instructions: Option<Instructions>,
    pub skills: Vec<Skill>,
    pub templates: Vec<Template>,
    pub problems: Vec<Problem>,
    /// Hosts predating MCP listing omit this list.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}
