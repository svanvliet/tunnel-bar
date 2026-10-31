//! Detects `devtunnel connect|host <id>` processes not started by this app.

use crate::model::{Forward, Role};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalConn {
    pub pid: u32,
    pub role: Role,
    /// Tunnel id as passed on the command line (may lack the ".cluster" suffix, or be empty for the default tunnel).
    pub tunnel_arg: String,
}

/// Extracts the tunnel argument from a devtunnel command line, if it is a `connect` invocation.
#[cfg(test)]
pub fn parse_cmdline(args: &[String]) -> Option<String> {
    parse_cmdline_role(args).filter(|(r, _)| *r == Role::Connect).map(|(_, a)| a)
}

/// Extracts the role and tunnel argument from a devtunnel `connect` or `host` command line.
pub fn parse_cmdline_role(args: &[String]) -> Option<(Role, String)> {
    let exe = args.first()?;
    let exe_name = std::path::Path::new(exe).file_name()?.to_string_lossy().to_lowercase();
    if !exe_name.contains("devtunnel") {
        return None;
    }
    let mut it = args.iter().skip(1);
    let role = it.by_ref().find_map(|a| match a.as_str() {
        "connect" => Some(Role::Connect),
        "host" => Some(Role::Host),
        _ => None,
    })?;
    let takes_value: &[&str] = match role {
        Role::Connect => &["-t", "--access-token"],
        Role::Host => &[
            "-t", "--access-token", "-p", "--port-numbers", "--protocol", "-e", "--expiration", "-l", "--labels", "-d",
            "--description",
        ],
    };
    let mut skip_value = false;
    for a in it {
        if skip_value {
            skip_value = false;
            continue;
        }
        if takes_value.contains(&a.as_str()) {
            skip_value = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        return Some((role, a.clone()));
    }
    Some((role, String::new()))
}

/// Whether an external tunnel argument refers to the given account tunnel id ("name.cluster").
pub fn matches_tunnel(arg: &str, tunnel_id: &str) -> bool {
    !arg.is_empty()
        && (arg.eq_ignore_ascii_case(tunnel_id)
            || tunnel_id.to_lowercase().strip_prefix(&format!("{}.", arg.to_lowercase())).is_some_and(|rest| !rest.contains('.')))
}

pub fn scan(exclude_pids: &[u32]) -> Vec<ExternalConn> {
    scan_all(exclude_pids).found
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub found: Vec<ExternalConn>,
    /// devtunnel processes whose command line can't be read (on Windows: started from an elevated terminal).
    pub opaque: Vec<u32>,
}

/// A process hosting a tunnel is never also its client, even when it listens on a port
/// (the host's local server) or was attributed rather than parsed.
pub fn exclusive_roles(found: &mut Vec<ExternalConn>) {
    let hosts: Vec<u32> = found.iter().filter(|e| e.role == Role::Host).map(|e| e.pid).collect();
    found.retain(|e| e.role == Role::Host || !hosts.contains(&e.pid));
}

pub fn scan_all(exclude_pids: &[u32]) -> ScanResult {
    let Ok(mut sys) = System::new() else { return ScanResult::default() };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let mut found = Vec::new();
    let mut opaque = Vec::new();
    for (pid, p) in sys.processes() {
        let pid = pid.as_u32();
        if exclude_pids.contains(&pid) {
            continue;
        }
        // Skip children of managed processes (wrapper -> real binary).
        if p.parent().is_some_and(|pp| exclude_pids.contains(&pp.as_u32())) {
            continue;
        }
        #[allow(unused_mut)]
        let mut args: Vec<String> = p.cmd().iter().map(|s| s.to_string_lossy().into_owned()).collect();
        // sysinfo reads the command line from the process's memory, which an elevated process denies us.
        #[cfg(windows)]
        if args.is_empty() && p.name().to_string_lossy().to_lowercase().starts_with("devtunnel") {
            match crate::win::process_cmdline(pid) {
                Some(a) => args = a,
                None => {
                    opaque.push(pid);
                    continue;
                }
            }
        }
        if let Some((role, tunnel_arg)) = parse_cmdline_role(&args) {
            found.push(ExternalConn { pid, role, tunnel_arg });
        }
    }
    // A wrapper and the real binary can both match; keep the parent only.
    let pids: Vec<u32> = found.iter().map(|f| f.pid).chain(opaque.iter().copied()).collect();
    let parent_listed = |pid: u32| {
        let parent = sys.process(sysinfo::Pid::from_u32(pid)).and_then(|p| p.parent()).map(|p| p.as_u32());
        parent.is_some_and(|pp| pids.contains(&pp))
    };
    found.retain(|f| !parent_listed(f.pid));
    opaque.retain(|&p| !parent_listed(p));
    found.sort_by_key(|f| f.pid);
    opaque.sort_unstable();
    ScanResult { found, opaque }
}

#[derive(Debug, Default, PartialEq)]
pub struct Attribution {
    /// The unreadable process taken to be `devtunnel host <tunnel>`.
    pub host: Option<ExternalConn>,
    /// Unreadable devtunnel processes remain that couldn't be matched to a tunnel.
    pub unexplained: bool,
}

/// Matches devtunnel processes we can't inspect to tunnels the service reports as hosted.
/// `opaque` is (pid, has TCP listeners); `candidates` are tunnels with a host online that neither Tunnel Bar
/// nor a readable external process hosts. `devtunnel host` dials out and never listens, so listening processes
/// are clients (`connect`). Only a single host process with a single candidate tunnel is attributed.
pub fn attribute_opaque(opaque: &[(u32, bool)], candidates: &[String]) -> Attribution {
    let hosts: Vec<u32> = opaque.iter().filter(|(_, listening)| !listening).map(|(p, _)| *p).collect();
    let host = match (hosts.as_slice(), candidates) {
        ([pid], [id]) => Some(ExternalConn { pid: *pid, role: Role::Host, tunnel_arg: id.clone() }),
        _ => None,
    };
    Attribution { unexplained: opaque.len() > host.is_some() as usize, host }
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

/// Local TCP listen ports owned by a process or any of its descendants.
#[cfg(windows)]
pub fn listen_ports(pid: u32) -> Vec<u16> {
    let Ok(mut sys) = System::new() else { return vec![] };
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let parents: Vec<(u32, Option<u32>)> =
        sys.processes().iter().map(|(p, pr)| (p.as_u32(), pr.parent().map(|x| x.as_u32()))).collect();
    ports_owned_by(&crate::win::tcp_listeners(), &descendants(pid, &parents))
}

/// `root` plus every transitive child, in breadth-first order, given (pid, parent) pairs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn descendants(root: u32, parents: &[(u32, Option<u32>)]) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let cur = out[i];
        for &(p, pp) in parents {
            if pp == Some(cur) && p != cur && !out.contains(&p) {
                out.push(p);
            }
        }
        i += 1;
    }
    out
}

/// Sorted, de-duplicated ports from (port, pid) listener rows whose pid is in `pids`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ports_owned_by(listeners: &[(u16, u32)], pids: &[u32]) -> Vec<u16> {
    let mut ports: Vec<u16> = listeners.iter().filter(|(_, p)| pids.contains(p)).map(|(port, _)| *port).collect();
    ports.sort();
    ports.dedup();
    ports
}

#[cfg(any(unix, test))]
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
        assert_eq!(parse_cmdline_role(&v(&["devtunnel", "host", "x.usw3"])), Some((Role::Host, "x.usw3".into())));
        assert_eq!(
            parse_cmdline_role(&v(&["devtunnel", "host", "-p", "3000", "--protocol", "http"])),
            Some((Role::Host, String::new()))
        );
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

    #[test]
    fn walks_descendants_and_filters_listeners() {
        let tree = [(10, Some(1)), (11, Some(10)), (12, Some(11)), (20, Some(1)), (1, None)];
        assert_eq!(descendants(10, &tree), vec![10, 11, 12]);
        assert_eq!(descendants(99, &tree), vec![99]);
        let rows = [(2222, 11), (2222, 11), (8080, 20), (2223, 10)];
        assert_eq!(ports_owned_by(&rows, &[10, 11, 12]), vec![2222, 2223]);
    }

    #[test]
    fn parses_windows_cmdlines() {
        assert_eq!(
            parse_cmdline(&v(&[r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links\devtunnel.exe", "connect", "x.usw3"])),
            Some("x.usw3".into())
        );
        assert_eq!(parse_cmdline_role(&v(&["DevTunnel.EXE", "host", "y"])), Some((Role::Host, "y".into())));
    }

    #[test]
    fn attributes_a_single_unreadable_host() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(attribute_opaque(&[], &ids(&["a.usw3"])), Attribution::default());
        let one = attribute_opaque(&[(42, false)], &ids(&["a.usw3"]));
        assert_eq!(one.host, Some(ExternalConn { pid: 42, role: Role::Host, tunnel_arg: "a.usw3".into() }));
        assert!(!one.unexplained);
        // No tunnel is hosted without an owner we know of: nothing to attribute, but tell the user.
        assert_eq!(attribute_opaque(&[(42, false)], &[]), Attribution { host: None, unexplained: true });
        // Two candidate tunnels or two host processes: ambiguous.
        assert_eq!(attribute_opaque(&[(42, false)], &ids(&["a.usw3", "b.usw3"])), Attribution { host: None, unexplained: true });
        assert_eq!(attribute_opaque(&[(42, false), (43, false)], &ids(&["a.usw3"])), Attribution { host: None, unexplained: true });
        // A listening process is an elevated `connect`, not the host.
        assert_eq!(attribute_opaque(&[(42, true)], &ids(&["a.usw3"])), Attribution { host: None, unexplained: true });
        let mixed = attribute_opaque(&[(42, true), (43, false)], &ids(&["a.usw3"]));
        assert_eq!(mixed.host.map(|h| h.pid), Some(43));
        assert!(mixed.unexplained);
    }

    #[test]
    fn a_host_process_is_never_also_a_client() {
        let conn = |pid, role, t: &str| ExternalConn { pid, role, tunnel_arg: t.into() };
        // pid 7 hosts svv; a parse or attribution that also lists it as a client must not survive.
        let mut found = vec![conn(7, Role::Connect, "svv"), conn(9, Role::Connect, "other"), conn(7, Role::Host, "svv")];
        exclusive_roles(&mut found);
        assert_eq!(found, vec![conn(9, Role::Connect, "other"), conn(7, Role::Host, "svv")]);
        // A separate client of the same tunnel on this machine stays.
        let mut found = vec![conn(7, Role::Host, "svv"), conn(8, Role::Connect, "svv")];
        exclusive_roles(&mut found);
        assert_eq!(found.len(), 2);
    }
}
