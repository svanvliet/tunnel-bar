//! Detects `devtunnel connect <id>` processes not started by this app.

use crate::model::Forward;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalConn {
    pub pid: u32,
    /// Tunnel id as passed on the command line (may lack the ".cluster" suffix, or be empty for the default tunnel).
    pub tunnel_arg: String,
}

/// Extracts the tunnel argument from a devtunnel command line, if it is a `connect` invocation.
pub fn parse_cmdline(args: &[String]) -> Option<String> {
    let exe = args.first()?;
    let exe_name = std::path::Path::new(exe).file_name()?.to_string_lossy().to_lowercase();
    if !exe_name.contains("devtunnel") {
        return None;
    }
    let mut it = args.iter().skip(1);
    it.by_ref().find(|a| a.as_str() == "connect")?;
    let mut skip_value = false;
    for a in it {
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(a.as_str(), "-t" | "--access-token") {
            skip_value = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        return Some(a.clone());
    }
    Some(String::new())
}

/// Whether an external tunnel argument refers to the given account tunnel id ("name.cluster").
pub fn matches_tunnel(arg: &str, tunnel_id: &str) -> bool {
    !arg.is_empty()
        && (arg.eq_ignore_ascii_case(tunnel_id)
            || tunnel_id.to_lowercase().strip_prefix(&format!("{}.", arg.to_lowercase())).is_some_and(|rest| !rest.contains('.')))
}

pub fn scan(exclude_pids: &[u32]) -> Vec<ExternalConn> {
    let Ok(mut sys) = System::new() else { return vec![] };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let mut found = Vec::new();
    for (pid, p) in sys.processes() {
        let pid = pid.as_u32();
        if exclude_pids.contains(&pid) {
            continue;
        }
        // Skip children of managed processes (wrapper -> real binary).
        if p.parent().is_some_and(|pp| exclude_pids.contains(&pp.as_u32())) {
            continue;
        }
        let args: Vec<String> = p.cmd().iter().map(|s| s.to_string_lossy().into_owned()).collect();
        if let Some(tunnel_arg) = parse_cmdline(&args) {
            found.push(ExternalConn { pid, tunnel_arg });
        }
    }
    // A wrapper and the real binary can both match; keep the parent only.
    let pids: Vec<u32> = found.iter().map(|f| f.pid).collect();
    found.retain(|f| {
        let parent = sys.process(sysinfo::Pid::from_u32(f.pid)).and_then(|p| p.parent()).map(|p| p.as_u32());
        !parent.is_some_and(|pp| pids.contains(&pp))
    });
    found.sort_by_key(|f| f.pid);
    found
}

/// Best-effort list of local TCP listen ports for a process (and its children) via lsof.
#[cfg(unix)]
pub fn listen_ports(pid: u32) -> Vec<u16> {
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-g", &pid_group(pid), "-Fn"])
        .output();
    let Ok(out) = out else { return vec![] };
    parse_lsof_ports(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(unix)]
fn pid_group(pid: u32) -> String {
    let pgid = unsafe { libc::getpgid(pid as i32) };
    if pgid > 0 { pgid.to_string() } else { pid.to_string() }
}

#[cfg(windows)]
pub fn listen_ports(_pid: u32) -> Vec<u16> {
    vec![]
}

pub fn parse_lsof_ports(s: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = s
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter_map(|n| n.rsplit(':').next()?.parse().ok())
        .collect();
    ports.sort();
    ports.dedup();
    ports
}

/// Pairs external listen ports with the tunnel's remote ports when unambiguous.
pub fn guess_forwards(local: &[u16], remote: &[u16]) -> Vec<Forward> {
    if local.len() == 1 && remote.len() == 1 {
        return vec![Forward { local_port: local[0], remote_port: remote[0] }];
    }
    local
        .iter()
        .map(|&l| Forward { local_port: l, remote_port: if remote.contains(&l) { l } else { 0 } })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_cmdlines() {
        assert_eq!(parse_cmdline(&v(&["devtunnel", "connect", "devbox-ssh"])), Some("devbox-ssh".into()));
        assert_eq!(
            parse_cmdline(&v(&["/opt/homebrew/bin/osx-arm64-devtunnel", "connect", "-t", "tok", "x.usw3", "-v"])),
            Some("x.usw3".into())
        );
        assert_eq!(parse_cmdline(&v(&["devtunnel", "connect"])), Some(String::new()));
        assert_eq!(parse_cmdline(&v(&["devtunnel", "host", "x"])), None);
        assert_eq!(parse_cmdline(&v(&["ssh", "connect", "x"])), None);
    }

    #[test]
    fn matches_ids() {
        assert!(matches_tunnel("devbox-ssh", "devbox-ssh.usw3"));
        assert!(matches_tunnel("devbox-ssh.usw3", "devbox-ssh.usw3"));
        assert!(!matches_tunnel("devbox", "devbox-ssh.usw3"));
        assert!(!matches_tunnel("", "x.usw3"));
    }

    #[test]
    fn parses_lsof() {
        assert_eq!(parse_lsof_ports("p2787\nf125\nn127.0.0.1:2222\nf126\nn[::1]:2222\n"), vec![2222]);
        assert_eq!(guess_forwards(&[2223], &[2222]), vec![Forward { local_port: 2223, remote_port: 2222 }]);
    }
}
