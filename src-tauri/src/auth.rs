//! Startup/sign-in state machine: classifies `devtunnel user show` and login output into an explicit
//! `AuthState`, so the UI never has to infer "signed out" from missing data.

use crate::model::{AuthState, LoginFailure, LoginProvider};
use regex::Regex;
use std::sync::LazyLock;

/// Result of one `devtunnel user show`.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthCheck {
    SignedIn { message: String, provider: Option<LoginProvider> },
    SignedOut { message: String },
    /// The service couldn't be reached (DNS, connection, TLS or timeout).
    Offline { message: String },
    /// Anything else the CLI did that we don't understand.
    Error { message: String },
}

static SIGNED_OUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(not logged in|login required|please log ?in|devtunnel user login|unauthorized|\b401\b|(access|refresh)? ?token.*(expired|invalid)|session.*expired)").unwrap()
});
static OFFLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(timed? ?out|no such host|name or service not known|nodename nor servname|could not be resolved|name resolution|network is unreachable|network unreachable|no route to host|connection (refused|reset|was closed|attempt failed)|unable to connect|httprequestexception|socketexception|ssl connection|offline|no internet|temporary failure)").unwrap()
});
static DENIED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(access (is )?denied|forbidden|\b403\b|not authori[sz]ed|blocked|disabled|conditional access|AADSTS(50105|50057|53003|530003|50020|500011|90072))").unwrap()
});

pub fn provider_from_message(message: &str) -> Option<LoginProvider> {
    let m = message.to_ascii_lowercase();
    let using = m.rsplit_once(" using ")?.1;
    if using.starts_with("github") {
        Some(LoginProvider::Github)
    } else if using.starts_with("microsoft") || using.starts_with("aad") || using.starts_with("msa") {
        Some(LoginProvider::Microsoft)
    } else {
        None
    }
}

/// Classifies the output of `devtunnel user show`.
pub fn classify(success: bool, stdout: &str, stderr: &str) -> AuthCheck {
    let text = format!("{}\n{}", stdout.trim(), stderr.trim()).trim().to_string();
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    if first.starts_with("Logged in as") {
        return AuthCheck::SignedIn { provider: provider_from_message(&first), message: first };
    }
    if SIGNED_OUT.is_match(&text) {
        return AuthCheck::SignedOut { message: first };
    }
    if OFFLINE.is_match(&text) {
        return AuthCheck::Offline { message: summary(&text) };
    }
    let message = if text.is_empty() {
        if success { "devtunnel printed nothing".into() } else { "devtunnel exited with an error and no output".into() }
    } else {
        summary(&text)
    };
    AuthCheck::Error { message }
}

/// Classifies a failure to run `devtunnel user show` at all (spawn error or our timeout).
pub fn classify_run_error(err: &str) -> AuthCheck {
    if OFFLINE.is_match(err) { AuthCheck::Offline { message: err.into() } } else { AuthCheck::Error { message: err.into() } }
}

fn summary(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("Request ID:")).take(6).collect();
    lines.join("\n")
}

/// Seconds before the next automatic retry after `attempt` consecutive offline checks (1-based).
pub fn backoff_secs(attempt: u32) -> u64 {
    (5u64 << attempt.saturating_sub(1).min(4)).min(60)
}

/// The next state after a check. `was_signed_in` is the persisted "last known signed in" flag, which
/// turns a signed-out result into `Expired`. A transient network or CLI error never knocks a signed-in
/// user back to an auth screen; the caller reports it as a refresh error instead.
pub fn transition(prev: &AuthState, check: &AuthCheck, was_signed_in: bool, now: chrono::DateTime<chrono::Local>) -> AuthState {
    match check {
        AuthCheck::SignedIn { .. } => AuthState::SignedIn,
        AuthCheck::SignedOut { .. } if was_signed_in || matches!(prev, AuthState::Expired) => AuthState::Expired,
        AuthCheck::SignedOut { .. } => AuthState::SignedOut,
        AuthCheck::Offline { .. } | AuthCheck::Error { .. } if matches!(prev, AuthState::SignedIn) => AuthState::SignedIn,
        AuthCheck::Offline { message } => {
            let attempt = match prev { AuthState::Offline { attempt, .. } => attempt + 1, _ => 1 };
            let secs = backoff_secs(attempt);
            AuthState::Offline {
                message: message.clone(),
                attempt,
                retry_in_secs: secs,
                retry_at: (now + chrono::Duration::seconds(secs as i64)).to_rfc3339(),
            }
        }
        AuthCheck::Error { message } => AuthState::Error { message: message.clone() },
    }
}

/// Why an interactive sign-in failed, from its final message.
pub fn login_failure(message: &str) -> LoginFailure {
    if message == "Sign-in cancelled" {
        LoginFailure::Cancelled
    } else if message == "Sign-in timed out" || message.to_ascii_lowercase().contains("expired_token") || message.contains("AADSTS70016") {
        LoginFailure::TimedOut
    } else if DENIED.is_match(message) {
        LoginFailure::Denied
    } else if OFFLINE.is_match(message) {
        LoginFailure::Offline
    } else {
        LoginFailure::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> chrono::DateTime<chrono::Local> {
        chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00+00:00").unwrap().with_timezone(&chrono::Local)
    }

    #[test]
    fn classifies_user_show() {
        assert_eq!(
            classify(true, "Logged in as dev@example.com using GitHub.\n", ""),
            AuthCheck::SignedIn { message: "Logged in as dev@example.com using GitHub.".into(), provider: Some(LoginProvider::Github) }
        );
        assert!(matches!(classify(true, "Logged in as dev@example.com using Microsoft.", ""), AuthCheck::SignedIn { provider: Some(LoginProvider::Microsoft), .. }));
        assert!(matches!(classify(true, "Not logged in.\n", ""), AuthCheck::SignedOut { .. }));
        assert!(matches!(classify(false, "", "Error: Access token expired. Please log in again."), AuthCheck::SignedOut { .. }));
        assert!(matches!(classify(false, "", "Error: No such host is known. (global.rel.tunnels.api.visualstudio.com:443)"), AuthCheck::Offline { .. }));
        assert!(matches!(classify(false, "", "System.Net.Http.HttpRequestException: nodename nor servname provided, or not known"), AuthCheck::Offline { .. }));
        assert!(matches!(classify(false, "Unhandled exception: example\n   at Example.Main()", ""), AuthCheck::Error { .. }));
        assert!(matches!(classify(false, "", ""), AuthCheck::Error { .. }));
        assert!(matches!(classify_run_error("`devtunnel user show` timed out"), AuthCheck::Offline { .. }));
        assert!(matches!(classify_run_error("failed to run devtunnel: permission denied"), AuthCheck::Error { .. }));
    }

    #[test]
    fn error_summary_drops_request_ids_and_caps_lines() {
        let text = (0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n") + "\nRequest ID: abc";
        let AuthCheck::Error { message } = classify(false, &text, "") else { panic!() };
        assert_eq!(message.lines().count(), 6);
        assert!(!message.contains("Request ID"));
    }

    #[test]
    fn startup_transitions() {
        let ok = AuthCheck::SignedIn { message: "Logged in as a@example.com using GitHub.".into(), provider: None };
        let out = AuthCheck::SignedOut { message: "Not logged in.".into() };
        assert_eq!(transition(&AuthState::Checking, &ok, false, at()), AuthState::SignedIn);
        assert_eq!(transition(&AuthState::Checking, &out, false, at()), AuthState::SignedOut);
        assert_eq!(transition(&AuthState::Checking, &out, true, at()), AuthState::Expired);
        assert_eq!(transition(&AuthState::Expired, &out, false, at()), AuthState::Expired);
        assert_eq!(transition(&AuthState::CliMissing, &ok, true, at()), AuthState::SignedIn);
    }

    #[test]
    fn offline_backs_off_and_never_signs_out() {
        let off = AuthCheck::Offline { message: "timed out".into() };
        let first = transition(&AuthState::Checking, &off, true, at());
        let AuthState::Offline { attempt: 1, retry_in_secs: 5, ref retry_at, .. } = first else { panic!("{first:?}") };
        assert!(retry_at.starts_with("2026-01-01") && retry_at.contains(":00:05"));
        let mut s = first.clone();
        let mut delays = vec![];
        for _ in 0..6 {
            s = transition(&s, &off, true, at());
            let AuthState::Offline { retry_in_secs, .. } = s else { panic!() };
            delays.push(retry_in_secs);
        }
        assert_eq!(delays, [10, 20, 40, 60, 60, 60]);
        assert_eq!(transition(&AuthState::SignedIn, &off, true, at()), AuthState::SignedIn);
        assert_eq!(transition(&AuthState::SignedIn, &AuthCheck::Error { message: "x".into() }, true, at()), AuthState::SignedIn);
        assert_eq!(transition(&s, &AuthCheck::SignedIn { message: String::new(), provider: None }, true, at()), AuthState::SignedIn);
        assert_eq!(transition(&AuthState::Checking, &AuthCheck::Error { message: "boom".into() }, false, at()), AuthState::Error { message: "boom".into() });
    }

    #[test]
    fn login_failures() {
        assert_eq!(login_failure("Sign-in cancelled"), LoginFailure::Cancelled);
        assert_eq!(login_failure("Sign-in timed out"), LoginFailure::TimedOut);
        assert_eq!(login_failure("Error: AADSTS70016 authorization_pending expired"), LoginFailure::TimedOut);
        assert_eq!(login_failure("Error: AADSTS53003: Access has been blocked by Conditional Access policies."), LoginFailure::Denied);
        assert_eq!(login_failure("Error: No such host is known."), LoginFailure::Offline);
        assert_eq!(login_failure("Sign-in failed"), LoginFailure::Error);
    }

    #[test]
    fn provider_parsing() {
        assert_eq!(provider_from_message("Logged in as a using GitHub."), Some(LoginProvider::Github));
        assert_eq!(provider_from_message("Logged in as a using Microsoft."), Some(LoginProvider::Microsoft));
        assert_eq!(provider_from_message("Logged in as a"), None);
    }
}
