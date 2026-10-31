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
}

/// A local listener forwarding to a remote tunnel port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub local_port: u16,
    pub remote_port: u16,
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
    /// A `devtunnel connect` started outside this app.
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
