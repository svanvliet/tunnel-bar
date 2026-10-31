use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tunnel summary as returned by `devtunnel list -j`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelSummary {
    pub tunnel_id: String,
    pub name: String,
    pub host_connections: u32,
    pub labels: Vec<String>,
    pub port_count: u32,
    pub tunnel_expiration: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelPort {
    pub port_number: u16,
    pub protocol: String,
    pub port_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccessEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub subjects: Vec<String>,
    pub scopes: Vec<String>,
    #[serde(alias = "deny")]
    pub is_deny: bool,
}

impl AccessEntry {
    pub fn allows_anonymous(&self) -> bool {
        self.kind.eq_ignore_ascii_case("anonymous")
            && !self.is_deny
            && (self.scopes.is_empty() || self.scopes.iter().any(|s| s == "connect"))
    }
}

/// Tunnel details as returned by `devtunnel show <id> -j`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelDetails {
    pub tunnel_id: String,
    pub host_connections: u32,
    pub client_connections: u32,
    pub labels: Vec<String>,
    pub description: String,
    pub ports: Vec<TunnelPort>,
    pub access_control: Vec<AccessEntry>,
}

/// A local listener forwarding to a remote tunnel port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub local_port: u16,
    pub remote_port: u16,
}

/// Which `devtunnel` subcommand a supervised process runs.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Connect,
    Host,
}

impl Role {
    pub fn subcommand(self) -> &'static str {
        match self {
            Role::Connect => "connect",
            Role::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    /// Process exited unexpectedly; waiting before the next attempt.
    Backoff { attempt: u32, retry_in_secs: u64, message: String },
    AuthRequired { message: String },
    /// A `devtunnel connect`/`host` started outside this app.
    External { pid: u32 },
}

impl ConnState {
    pub fn is_up(&self) -> bool {
        matches!(self, ConnState::Connected | ConnState::External { .. })
    }
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            ConnState::Backoff { .. } | ConnState::AuthRequired { .. }
        )
    }
    pub fn is_transitional(&self) -> bool {
        matches!(self, ConnState::Connecting | ConnState::Reconnecting)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelSettings {
    pub nickname: Option<String>,
    pub auto_connect: bool,
    /// Host this tunnel when Tunnel Bar starts.
    pub auto_host: bool,
    /// ~/.ssh/config Host alias to use for "Open SSH" (port is overridden with -p).
    pub ssh_host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub cli_path: Option<String>,
    pub launch_at_login: bool,
    pub notifications: bool,
    pub first_run_done: bool,
    pub tunnels: BTreeMap<String, TunnelSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cli_path: None,
            launch_at_login: true,
            notifications: true,
            first_run_done: false,
            tunnels: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub id: String,
    pub display_name: String,
    pub summary: TunnelSummary,
    pub ports: Vec<TunnelPort>,
    pub state: ConnState,
    pub forwards: Vec<Forward>,
    pub settings: TunnelSettings,
    /// Tunnel is running but no longer listed on the account.
    pub orphaned: bool,
    pub host_state: ConnState,
    /// Ports published by our `devtunnel host`, with public URLs.
    pub hosted_ports: Vec<HostedPort>,
    pub anonymous: bool,
    /// Created by Tunnel Bar's "Expose local port" (label `tunnelbar`).
    pub created_by_app: bool,
    /// A host other than this app's is connected to the relay.
    pub hosted_elsewhere: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostedPort {
    pub port: u16,
    pub protocol: String,
    pub uri: String,
    pub inspect_uri: String,
    /// Something is accepting connections on localhost:port.
    pub listening: bool,
}

pub const APP_LABEL: &str = "tunnelbar";

/// "https://abc-3000.usw3.devtunnels.ms/" -> "https://abc-3000-inspect.usw3.devtunnels.ms/"
pub fn inspect_uri(port_uri: &str) -> String {
    let Some(rest) = port_uri.strip_prefix("https://") else { return String::new() };
    match rest.split_once('.') {
        Some((host, domain)) if !host.is_empty() => format!("https://{host}-inspect.{domain}"),
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    pub auth: Option<AuthStatus>,
    pub tunnels: Vec<TunnelView>,
    pub settings: Settings,
    pub last_refresh: Option<String>,
    pub refresh_error: Option<String>,
    pub refreshing: bool,
    pub log_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts: String,
    pub stream: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_inspect_uri() {
        assert_eq!(inspect_uri("https://abc-18765.usw3.devtunnels.ms/"), "https://abc-18765-inspect.usw3.devtunnels.ms/");
        assert_eq!(inspect_uri(""), "");
    }

    #[test]
    fn detects_anonymous_access() {
        let e: AccessEntry = serde_json::from_str(r#"{"type":"Anonymous","subjects":[],"scopes":["connect"]}"#).unwrap();
        assert!(e.allows_anonymous());
        let d: AccessEntry =
            serde_json::from_str(r#"{"type":"Anonymous","subjects":[],"scopes":["connect"],"isDeny":true}"#).unwrap();
        assert!(!d.allows_anonymous());
    }
}
