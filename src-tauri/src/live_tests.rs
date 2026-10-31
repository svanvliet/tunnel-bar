//! Live Phase 3 tests against the real CLI and signed-in account:
//! `cargo test live_manage -- --ignored --nocapture`.
//!
//! Only touches a temporary `tunnelbar-test-*` tunnel created here with a 1h expiration, and
//! always deletes it (also on panic), then checks `devtunnel list -j` no longer shows it.

use crate::cli;
use crate::logs::LogStore;
use crate::model::{AccessRuleKind, AccessRuleRequest, ConnState, HeaderOptions, Role, UpdateTunnelRequest};
use crate::pin::Pins;
use crate::supervisor::Supervisor;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PREFIX: &str = "tunnelbar-test-";

struct TempTunnel {
    cli: PathBuf,
    id: String,
}

impl Drop for TempTunnel {
    fn drop(&mut self) {
        assert!(self.id.starts_with(PREFIX), "refusing to delete {}", self.id);
        let out = std::process::Command::new(&self.cli).args(["delete", &self.id, "-f"]).output();
        eprintln!("cleanup delete {}: {:?}", self.id, out.map(|o| o.status));
        let list = std::process::Command::new(&self.cli).args(["list", "-j"]).output().expect("devtunnel list -j");
        let tunnels = cli::parse_list(&String::from_utf8_lossy(&list.stdout)).expect("parse list");
        let ids: Vec<_> = tunnels.iter().map(|t| t.tunnel_id.as_str()).collect();
        eprintln!("devtunnel list -j after cleanup: {ids:?}");
        assert!(!ids.iter().any(|t| t.starts_with(PREFIX)), "test tunnel left behind: {ids:?}");
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn http_server(body: &'static str) -> u16 {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = c.read(&mut buf);
            let _ = write!(c, "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        }
    });
    port
}

fn get(port: u16) -> String {
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    c.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").unwrap();
    let mut s = String::new();
    let _ = c.read_to_string(&mut s);
    s
}

fn unchanged() -> HeaderOptions {
    HeaderOptions { host_header: Some("unchanged".into()), origin_header: Some("unchanged".into()) }
}

fn wait(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..600 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
#[ignore]
fn live_manage_lifecycle_and_pin() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cli = rt.block_on(cli::find_cli(None)).expect("devtunnel CLI");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() % 1_000_000_000;
    let name = format!("{PREFIX}{nonce}");

    // Create with labels, description and the 1h expiration.
    let id = rt.block_on(cli::create(&cli, Some(&name), &["tunnelbar", "tbtest"], "phase3 live", Some("1h"), &unchanged())).unwrap();
    let _guard = TempTunnel { cli: cli.clone(), id: id.clone() };
    assert!(id.starts_with(&name), "{id}");
    let d = rt.block_on(cli::show(&cli, &id)).unwrap();
    assert_eq!(crate::model::parse_expiration_hours(&d.tunnel_expiration), Some(1), "{}", d.tunnel_expiration);
    assert!(d.labels.contains(&"tbtest".to_string()));

    // Edit labels, description and expiration.
    let req = UpdateTunnelRequest {
        description: Some("edited".into()),
        add_labels: vec!["alpha".into(), "beta".into()],
        remove_labels: vec!["tbtest".into()],
        expiration: Some("2h".into()),
        headers: Default::default(),
    };
    rt.block_on(cli::update(&cli, &id, &req)).unwrap();
    let ok = wait(|| {
        let d = rt.block_on(cli::show(&cli, &id)).unwrap();
        d.description == "edited"
            && d.labels.contains(&"alpha".into())
            && d.labels.contains(&"beta".into())
            && !d.labels.contains(&"tbtest".into())
            && crate::model::parse_expiration_hours(&d.tunnel_expiration) == Some(2)
    });
    assert!(ok, "update visible: {:?}", rt.block_on(cli::show(&cli, &id)));

    // Add two ports, remove one.
    let served = http_server("tunnelbar-pin-ok");
    let extra = free_port();
    rt.block_on(cli::port_create(&cli, &id, served, "http", &Default::default())).unwrap();
    let custom = HeaderOptions { host_header: Some("localhost".into()), origin_header: Some("https://example.com".into()) };
    rt.block_on(cli::port_create(&cli, &id, extra, "auto", &custom)).unwrap();
    let hdrs = |port: u16| {
        let d = rt.block_on(cli::show(&cli, &id)).unwrap();
        d.ports.iter().find(|p| p.port_number == port).map(|p| (p.host_header.clone(), p.origin_header.clone()))
    };
    let some = |s: &str| Some(s.to_string());
    // Ports inherit the tunnel-level `unchanged` unless they set their own.
    assert!(wait(|| hdrs(served) == Some((some("unchanged"), some("unchanged")))), "inherited: {:?}", hdrs(served));
    assert!(wait(|| hdrs(extra) == Some((some("localhost"), some("https://example.com")))), "custom: {:?}", hdrs(extra));
    let host_only = HeaderOptions { host_header: Some("unchanged".into()), origin_header: None };
    rt.block_on(cli::port_update_headers(&cli, &id, extra, &host_only)).unwrap();
    assert!(wait(|| hdrs(extra).is_some_and(|h| h.0 == some("unchanged"))), "updated: {:?}", hdrs(extra));

    // Tenant access rule with an expiration on the tunnel and a deny-anonymous on a port; list, then remove.
    let tenant = AccessRuleRequest { kind: AccessRuleKind::Tenant, expiration: Some("2h".into()), ..Default::default() };
    rt.block_on(cli::access_create(&cli, &id, &tenant.normalized().unwrap())).unwrap();
    let rules = rt.block_on(cli::access_list(&cli, &id, None)).unwrap();
    eprintln!("tunnel rules: {rules:?}");
    let t = rules.iter().find(|e| e.kind == "Organizations").expect("tenant rule");
    assert_eq!(t.index, Some(0));
    assert!(t.expiration.is_some(), "expiration on rule");
    let deny = AccessRuleRequest { kind: AccessRuleKind::Anonymous, deny: true, port: Some(extra), ..Default::default() };
    rt.block_on(cli::access_create(&cli, &id, &deny)).unwrap();
    let port_rules = rt.block_on(cli::access_list(&cli, &id, Some(extra))).unwrap();
    eprintln!("port rules: {port_rules:?}");
    assert!(port_rules.iter().any(|e| e.is_inherited && e.kind == "Organizations"));
    let own: Vec<_> = port_rules.iter().filter(|e| !e.is_inherited).collect();
    assert_eq!(own.len(), 1);
    assert!(own[0].is_deny && own[0].index == Some(0));
    rt.block_on(cli::access_delete(&cli, &id, Some(extra), 0)).unwrap();
    assert!(rt.block_on(cli::access_list(&cli, &id, Some(extra))).unwrap().iter().all(|e| e.is_inherited));
    rt.block_on(cli::access_delete(&cli, &id, None, 0)).unwrap();
    assert!(rt.block_on(cli::access_list(&cli, &id, None)).unwrap().is_empty());

    rt.block_on(cli::port_delete(&cli, &id, extra)).unwrap();
    assert!(wait(|| {
        let ports: Vec<u16> = rt.block_on(cli::show(&cli, &id)).unwrap().ports.iter().map(|p| p.port_number).collect();
        ports == vec![served]
    }));

    // Host it, then connect through a pinned local port.
    let dir = tempfile::tempdir().unwrap();
    let logs = Arc::new(LogStore::new(dir.path().join("logs")));
    let host = Supervisor::with_role(Role::Host, logs.clone(), Arc::new(|_| {}));
    host.connect(&id, cli.clone());
    assert!(wait(|| host.state(&id).0 == ConnState::Connected && !host.state(&id).1.is_empty()), "hosted");
    std::thread::sleep(Duration::from_secs(2));

    let client = Arc::new(Supervisor::new(logs, Arc::new(|_| {})));
    let c2 = client.clone();
    let pins = Pins::new(Arc::new(move |tid: &str, remote: u16| {
        c2.state(tid).1.iter().find(|f| f.remote_port == remote).map(|f| f.local_port)
    }));
    let pinned = free_port();
    pins.sync(&[(id.clone(), served, pinned)]);
    assert!(wait(|| pins.view(&id, served, pinned).listening), "pin listening");
    client.connect(&id, cli.clone());
    assert!(wait(|| !client.state(&id).1.is_empty()), "client forwarding");
    let chosen = client.state(&id).1[0].local_port;
    eprintln!("served={served} pinned={pinned} connect chose={chosen}");
    assert_ne!(chosen, pinned);
    let body = get(pinned);
    eprintln!("response head={:?}", body.lines().next());
    assert!(body.contains("tunnelbar-pin-ok"), "HTTP through the pin: {body:?}");

    let pids = [host.managed_pids(), client.managed_pids()].concat();
    pins.stop_all();
    client.disconnect(&id);
    host.disconnect(&id);
    assert!(wait(|| host.state(&id).0 == ConnState::Disconnected));
    std::thread::sleep(Duration::from_millis(500));
    assert!(pids.iter().all(|&p| !crate::supervisor::alive(p)));

    rt.block_on(cli::delete(&cli, &id)).unwrap();
}

#[test]
#[ignore]
fn live_manage_host_port_map() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cli = rt.block_on(cli::find_cli(None)).expect("devtunnel CLI");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() % 1_000_000_000;
    let name = format!("{PREFIX}map-{nonce}");
    let id = rt.block_on(cli::create(&cli, Some(&name), &["tunnelbar"], "port map live", Some("1h"), &Default::default())).unwrap();
    let _guard = TempTunnel { cli: cli.clone(), id: id.clone() };

    // The real server is on `local`; the tunnel advertises a different `remote` port.
    let local = http_server("tunnelbar-map-ok");
    let remote = free_port();
    assert_ne!(local, remote);
    assert_eq!(crate::pin::port_in_use(remote), None);
    rt.block_on(cli::port_create(&cli, &id, remote, "http", &Default::default())).unwrap();

    // Host-side proxy, as AppState::sync_host_maps runs it: listen on the tunnel port, forward to local.
    let id2 = id.clone();
    let maps = Pins::new(Arc::new(move |tid: &str, r: u16| (tid == id2 && r == remote).then_some(local)));
    maps.sync(&[(id.clone(), remote, remote)]);
    assert!(wait(|| maps.view(&id, remote, remote).listening), "map proxy listening");

    let dir = tempfile::tempdir().unwrap();
    let logs = Arc::new(LogStore::new(dir.path().join("logs")));
    let host = Supervisor::with_role(Role::Host, logs.clone(), Arc::new(|_| {}));
    host.connect(&id, cli.clone());
    assert!(wait(|| host.state(&id).0 == ConnState::Connected && !host.state(&id).1.is_empty()), "hosted");
    std::thread::sleep(Duration::from_secs(2));

    let client = Supervisor::new(logs, Arc::new(|_| {}));
    client.connect(&id, cli.clone());
    assert!(wait(|| client.state(&id).1.iter().any(|f| f.remote_port == remote)), "client forwarding");
    let via = client.state(&id).1.iter().find(|f| f.remote_port == remote).unwrap().local_port;
    eprintln!("local server={local} tunnel port={remote} client forward={via}");
    assert_ne!(via, local);
    let body = get(via);
    eprintln!("response head={:?}", body.lines().next());
    assert!(body.contains("tunnelbar-map-ok"), "HTTP through tunnel port {remote} -> localhost:{local}: {body:?}");

    let pids = [host.managed_pids(), client.managed_pids()].concat();
    client.disconnect(&id);
    host.disconnect(&id);
    maps.stop_all();
    assert!(wait(|| host.state(&id).0 == ConnState::Disconnected));
    std::thread::sleep(Duration::from_millis(500));
    assert!(pids.iter().all(|&p| !crate::supervisor::alive(p)));
    rt.block_on(cli::delete(&cli, &id)).unwrap();
}
