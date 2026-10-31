//! Parses `devtunnel connect` output lines into events.
//!
//! The CLI output is not a stable API, so unknown lines are ignored (they are still logged).
//! Message strings mirror the dev-tunnels SDK traces (TunnelRelayConnection, RetryTcpListenerFactory).

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliEvent {
    Connected { name: String },
    Forward { local_addr: String, local_port: u16, remote_port: u16 },
    Reconnecting,
    Restored,
    Closed { message: String },
    AuthRequired { message: String },
    Error { message: String },
}

static CONNECTED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^Connected to tunnel:\s*(?P<name>\S*)").unwrap());
// e.g. "SSH: Forwarding from 127.0.0.1:2223 to host port 2222." / "[::1]:2223"
static FORWARD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"Forwarding from (?P<addr>\[[0-9a-fA-F:.]+\]|[^\s:]+):(?P<lp>\d+) to host port (?P<rp>\d+)")
        .unwrap()
});
static AUTH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(not logged in|login required|please log ?in|devtunnel user login|unauthorized|\b401\b|access token.*(expired|invalid)|forbidden|\b403\b)",
    )
    .unwrap()
});
static ERROR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(error|fatal|unhandled exception)\b|tunnel not found|failed to").unwrap());

pub fn parse_line(raw: &str) -> Option<CliEvent> {
    let line = raw.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(c) = CONNECTED.captures(line) {
        return Some(CliEvent::Connected { name: c["name"].to_string() });
    }
    if let Some(c) = FORWARD.captures(line) {
        let (Ok(lp), Ok(rp)) = (c["lp"].parse(), c["rp"].parse()) else {
            return None;
        };
        return Some(CliEvent::Forward {
            local_addr: c["addr"].trim_matches(|ch| ch == '[' || ch == ']').to_string(),
            local_port: lp,
            remote_port: rp,
        });
    }
    if line.contains("tunnel relay restored") {
        return Some(CliEvent::Restored);
    }
    if line.contains("tunnel relay closed") {
        if line.contains("Reconnecting") {
            return Some(CliEvent::Reconnecting);
        }
        return Some(CliEvent::Closed { message: line.to_string() });
    }
    if AUTH.is_match(line) {
        return Some(CliEvent::AuthRequired { message: line.to_string() });
    }
    if ERROR.is_match(line) {
        return Some(CliEvent::Error { message: line.to_string() });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/connect_ok.txt");

    #[test]
    fn parses_real_connect_output() {
        let events: Vec<_> = FIXTURE.lines().filter_map(parse_line).collect();
        assert_eq!(
            events,
            vec![
                CliEvent::Restored,
                CliEvent::Connected { name: "devbox-ssh".into() },
                CliEvent::Forward { local_addr: "127.0.0.1".into(), local_port: 2223, remote_port: 2222 },
                CliEvent::Forward { local_addr: "::1".into(), local_port: 2223, remote_port: 2222 },
            ]
        );
    }

    #[test]
    fn parses_reconnect_and_close() {
        assert_eq!(
            parse_line("Connection to client tunnel relay closed. Reason: network. Reconnecting."),
            Some(CliEvent::Reconnecting)
        );
        assert!(matches!(
            parse_line("Connection to client tunnel relay closed. Tunnel deleted."),
            Some(CliEvent::Closed { .. })
        ));
    }

    #[test]
    fn detects_auth_and_errors() {
        assert!(matches!(
            parse_line("Login required. Run `devtunnel user login`."),
            Some(CliEvent::AuthRequired { .. })
        ));
        assert!(matches!(
            parse_line("Tunnel service returned 401 Unauthorized"),
            Some(CliEvent::AuthRequired { .. })
        ));
        assert!(matches!(parse_line("Error: Tunnel not found."), Some(CliEvent::Error { .. })));
        assert_eq!(parse_line("SSH: PortForwardingService listening on 127.0.0.1:2223."), None);
        assert_eq!(parse_line("   "), None);
    }
}
