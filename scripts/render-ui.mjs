import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const output = path.resolve(process.env.UI_SCREENSHOT_DIR ?? path.join(root, "docs/screenshots"));
const executable = process.env.BROWSER_EXECUTABLE;
if (!executable) throw new Error("Set BROWSER_EXECUTABLE to an installed Chromium executable. This script never installs or launches a headed browser.");
const checks = [];
const fixture = {
  cliPath: "/example/bin/devtunnel", cliVersion: "1.0.0", refreshing: false,
  auth: { loggedIn: true, message: "Logged in as developer@example.com using Microsoft." },
  lastRefresh: new Date().toISOString(), refreshError: null, logDir: "/example/logs/TunnelBar",
  settings: { cliPath: null, launchAtLogin: true, notifications: true, firstRunDone: true, tunnels: {}, hotkey: "CmdOrCtrl+Alt+Shift+T", labelFilter: [] },
  allLabels: ["development", "preview", "tunnelbar"], login: null, hotkeyError: null,
  tunnels: [
    {
      id: "example-ssh.test", displayName: "Development SSH", createdByApp: false,
      summary: { tunnelId: "example-ssh.test", name: "Development SSH", hostConnections: 1, labels: [], portCount: 1, tunnelExpiration: "", description: "" },
      state: { kind: "connected" }, hostState: { kind: "disconnected" },
      forwards: [{ localPort: 2222, remotePort: 2222 }], hostedPorts: [],
      ports: [{ portNumber: 2222, protocol: "ssh", portUri: "" }],
      settings: { autoConnect: true, autoHost: false, nickname: null, sshHost: null },
      anonymous: false, hostedElsewhere: true, orphaned: false, matchesFilter: true, pins: [], accessRules: [], expirationHours: 720,
    },
    {
      id: "example-preview.test", displayName: "localhost:5713", createdByApp: true,
      summary: { tunnelId: "example-preview.test", name: "localhost:5713", hostConnections: 1, labels: ["tunnelbar"], portCount: 1, tunnelExpiration: "", description: "" },
      state: { kind: "disconnected" }, hostState: { kind: "connected" }, forwards: [],
      hostedPorts: [{ port: 5713, protocol: "http", uri: "https://example-preview-5713.region.devtunnels.example/", inspectUri: "https://inspect.example.invalid/", listening: false }],
      ports: [{ portNumber: 5713, protocol: "http", portUri: "https://example-preview-5713.region.devtunnels.example/" }],
      settings: { autoConnect: false, autoHost: false, nickname: null, sshHost: null },
      anonymous: true, hostedElsewhere: false, orphaned: false, matchesFilter: true, pins: [], accessRules: [{type:"Anonymous", provider:"", subjects:[], scopes:[], isDeny:false, isInherited:false, index:0}], expirationHours: 168,
    },
    {
      id: "example-local.test", displayName: "localhost:22", createdByApp: true,
      summary: { tunnelId: "example-local.test", name: "localhost:22", hostConnections: 0, labels: ["tunnelbar"], portCount: 1, tunnelExpiration: "", description: "" },
      state: { kind: "disconnected" }, hostState: { kind: "disconnected" }, forwards: [], hostedPorts: [],
      ports: [{ portNumber: 22, protocol: "auto", portUri: "" }],
      settings: { autoConnect: false, autoHost: false, nickname: null, sshHost: null },
      anonymous: false, hostedElsewhere: false, orphaned: false, matchesFilter: true, pins: [], accessRules: [], expirationHours: 24,
    },
  ],
};
for (const tunnel of fixture.tunnels) {
  tunnel.portMaps = [];
  tunnel.settings.pinnedPorts = {};
  tunnel.settings.hostPortMap = {};
  for (const port of tunnel.hostedPorts) port.localPort = port.port;
}

function installFakeIPC(initial) {
  let callbackId = 0, listenerId = 0;
  const callbacks = new Map(), listeners = new Map();
  const fake = window.fake = {
    snapshot: initial, calls: [], copied: "", failClipboard: false, failCommand: initial.failOnStart ?? "", errors: [],
    logs: [
      { ts: "2026-01-01T14:32:01Z", stream: "app", text: "Connecting to example-ssh.test" },
      { ts: "2026-01-01T14:32:02Z", stream: "stdout", text: "Forwarding localhost:2222 → :2222" },
      { ts: "2026-01-01T14:36:12Z", stream: "err", text: "Connection interrupted. Relay endpoint unavailable." },
      { ts: "2026-01-01T14:36:20Z", stream: "app", text: "Reconnected to tunnel." },
    ],
    emit(event, payload) {
      for (const listener of listeners.values()) if (listener.event === event) callbacks.get(listener.handler)?.({ event, id: listener.id, payload });
    },
    publish() { this.emit("snapshot", structuredClone(this.snapshot)); },
  };
  window.addEventListener("error", (event) => fake.errors.push(String(event.error ?? event.message)));
  window.addEventListener("unhandledrejection", (event) => fake.errors.push(String(event.reason)));
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, id) => listeners.delete(id) };
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { callbacks.set(++callbackId, callback); return callbackId; },
    async invoke(command, args = {}) {
      args = JSON.parse(JSON.stringify(args));
      fake.calls.push({ command, args: structuredClone(args) });
      if (fake.failCommand === command) throw new Error("Synthetic command failure");
      const t = fake.snapshot.tunnels.find((tunnel) => tunnel.id === args.id);
      if (["delete_tunnel", "add_port", "remove_port", "set_port_headers"].includes(command) || (command === "update_tunnel" && (args.update.hostHeader || args.update.originHeader))) {
        if (t.hostedElsewhere && !args.allowHostedElsewhere) throw "HOSTED_ELSEWHERE: Another device is hosting this tunnel. This change can interrupt it.";
      }
      if (["delete_tunnel", "remove_port", "set_anonymous", "add_access_rule", "remove_access_rule"].includes(command) || (command === "update_tunnel" && args.update.expiration)) {
        if (!t.createdByApp && !args.confirm) throw "CONFIRM_REQUIRED: Tunnel Bar did not create this tunnel. Confirm this change separately.";
      }
      switch (command) {
        case "get_snapshot": {
          const captured = structuredClone(fake.snapshot);
          if (fake.snapshot.simulateSnapshotRace) {
            await new Promise(resolve=>setTimeout(resolve,25));
            fake.snapshot.tunnels[0].displayName="Latest snapshot";
            fake.publish();
            await new Promise(resolve=>setTimeout(resolve,25));
          }
          return captured;
        }
        case "plugin:app|version": return "0.2.0";
        case "plugin:event|listen": listeners.set(++listenerId, { ...args, id: listenerId }); return listenerId;
        case "plugin:event|unlisten": return;
        case "plugin:clipboard-manager|write_text":
          if (fake.failClipboard) throw new Error("Clipboard unavailable. Try again.");
          fake.copied = args.text; return;
        case "ssh_command": return `ssh -p ${t.settings.pinnedPorts?.[args.remotePort] ?? args.localPort} localhost`;
        case "get_logs": return structuredClone(fake.logs);
        case "clear_logs": fake.logs = []; return;
        case "refresh": fake.publish(); return;
        case "update_tunnel_settings": t.settings = args.settings; t.displayName = args.settings.nickname || t.summary.name; fake.publish(); return;
        case "update_app_settings": Object.assign(fake.snapshot.settings, args); fake.publish(); return;
        case "update_tunnel":
          if ([...(args.update.addLabels ?? []), ...(args.update.removeLabels ?? [])].some(label=>label.toLowerCase()==="tunnelbar")) throw "The tunnelbar ownership label is reserved.";
          if (args.update.description != null) t.summary.description=args.update.description;
          t.summary.labels=[...new Set([...t.summary.labels.filter(label=>!args.update.removeLabels?.includes(label)), ...(args.update.addLabels ?? [])])];
          if (args.update.expiration) t.expirationHours = Number(args.update.expiration.slice(0,-1))*(args.update.expiration.endsWith("d")?24:1);
          if (args.update.hostHeader || args.update.originHeader) for (const p of t.ports) { if(args.update.hostHeader) p.hostHeader=args.update.hostHeader; if(args.update.originHeader) p.originHeader=args.update.originHeader; }
          fake.publish(); return;
        case "add_port": {
          const remote = args.remotePort ?? args.port;
          t.ports.push({portNumber:remote,localPort:remote!==args.port?args.port:null,protocol:args.protocol,portUri:"",hostHeader:args.hostHeader||null,originHeader:args.originHeader||null});
          if(remote!==args.port) t.settings.hostPortMap[remote]=args.port;
          fake.publish(); return;
        }
        case "remove_port": t.ports=t.ports.filter(port=>port.portNumber!==args.port); delete t.settings.pinnedPorts[args.port]; delete t.settings.hostPortMap[args.port]; t.pins=t.pins.filter(pin=>pin.remotePort!==args.port); t.portMaps=t.portMaps.filter(map=>map.remotePort!==args.port); fake.publish(); return;
        case "set_port_map":
          if(args.localPort==null) delete t.settings.hostPortMap[args.remotePort]; else t.settings.hostPortMap[args.remotePort]=args.localPort;
          t.ports.find(port=>port.portNumber===args.remotePort).localPort=args.localPort;
          for(const port of t.hostedPorts) if(port.port===args.remotePort) port.localPort=args.localPort??args.remotePort;
          t.portMaps=t.portMaps.filter(map=>map.remotePort!==args.remotePort);
          if(args.localPort!=null) t.portMaps.push({remotePort:args.remotePort,localPort:args.localPort,listening:t.hostState.kind==="connected",error:null});
          fake.publish(); return;
        case "set_port_headers": Object.assign(t.ports.find(port=>port.portNumber===args.port), args.headers); fake.publish(); return;
        case "list_access_rules":
          return structuredClone(args.port == null ? t.accessRules : [...t.accessRules.map(entry=>({...entry,isInherited:true,index:null})), ...(t.portRules?.[args.port] ?? [])]);
        case "add_access_rule": {
          const list = args.rule.port == null ? t.accessRules : ((t.portRules ??= {})[args.rule.port] ??= []);
          list.push({type:{anonymous:"Anonymous",tenant:"Organizations",org:"GitHubOrganization",repo:"GitHubRepository"}[args.rule.kind], provider:args.rule.kind==="org"?"github":"microsoft",subjects:args.rule.value?[args.rule.value]:[],scopes:[],isDeny:!!args.rule.deny,isInherited:false,expiration:args.rule.expiration??null,index:list.length});
          fake.publish(); return;
        }
        case "remove_access_rule": {
          const list=args.port==null?t.accessRules:t.portRules[args.port];
          list.splice(args.index,1); list.forEach((entry,index)=>(entry.index=index)); fake.publish(); return;
        }
        case "set_pinned_port":
          t.settings.pinnedPorts ??= {};
          if (args.localPort == null) { delete t.settings.pinnedPorts[args.remotePort]; t.pins=t.pins.filter(pin=>pin.remotePort!==args.remotePort); }
          else {
            if(fake.snapshot.tunnels.some(other=>Object.entries(other.settings.pinnedPorts??{}).some(([remote,local])=>local===args.localPort&&(other.id!==t.id||Number(remote)!==args.remotePort)))) throw "That local port is already pinned.";
            t.settings.pinnedPorts[args.remotePort]=args.localPort; t.pins=[...t.pins.filter(pin=>pin.remotePort!==args.remotePort),{remotePort:args.remotePort,localPort:args.localPort,listening:true,targetPort:t.forwards.find(f=>f.remotePort===args.remotePort)?.localPort??null,error:null}];
          }
          fake.publish(); return;
        case "set_hotkey":
          if (args.hotkey && !args.hotkey.includes("+")) throw "A modifier is required.";
          fake.snapshot.settings.hotkey=args.hotkey; fake.snapshot.hotkeyError=null; fake.publish(); return;
        case "set_label_filter": fake.snapshot.settings.labelFilter=args.labels; for(const tunnel of fake.snapshot.tunnels) tunnel.matchesFilter=!args.labels.length||tunnel.summary.labels.some(label=>args.labels.some(filter=>filter.toLowerCase()===label.toLowerCase())); fake.publish(); return;
        case "start_login": fake.snapshot.login=args.deviceCode?{kind:"deviceCode",provider:args.provider,url:"https://login.example.invalid/device",code:"ABCD-EFGH"}:{kind:"running",provider:args.provider}; fake.publish(); return;
        case "cancel_login": fake.snapshot.login=null; fake.publish(); return;
        case "logout": fake.snapshot.auth.loggedIn=false; fake.snapshot.tunnels=[]; fake.snapshot.login=null; fake.publish(); return;
        case "set_anonymous": t.anonymous = args.allow; fake.publish(); return;
        case "delete_tunnel": fake.snapshot.tunnels = fake.snapshot.tunnels.filter((tunnel) => tunnel.id !== args.id); fake.publish(); return;
        case "host": t.hostState = { kind: "connecting" }; fake.publish(); return;
        case "stop_host": t.hostState = { kind: "disconnected" }; t.hostedPorts = []; fake.publish(); return;
        case "connect": case "adopt_external": t.state = { kind: "connecting" }; fake.publish(); return;
        case "disconnect": case "stop_external": t.state = { kind: "disconnected" }; t.forwards = []; fake.publish(); return;
        case "expose_port": return args.name || "example-created.test";
        case "open_ssh": case "open_url": case "open_login": case "open_log_dir": case "quit": return;
        default: throw new Error("Unexpected command in UI: " + command);
      }
    },
  };
}

const server = createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const file = path.resolve(root, "build", "." + (pathname === "/" ? "/index.html" : pathname));
    if (!file.startsWith(path.join(root, "build") + path.sep)) { response.writeHead(403); response.end(); return; }
    const content = await readFile(file);
    const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".png": "image/png" };
    response.writeHead(200, { "content-type": types[path.extname(file)] ?? "application/octet-stream" });
    response.end(content);
  } catch (error) {
    if (error.code !== "ENOENT") console.error(error);
    response.writeHead(404); response.end("Not found");
  }
});
server.listen(0, "127.0.0.1");
await once(server, "listening");
const url = `http://127.0.0.1:${server.address().port}/`;
const profile = await mkdtemp(path.join(tmpdir(), "tunnel-bar-ui-"));
await mkdir(output, { recursive: true });
const browser = spawn(executable, ["--headless", "--disable-gpu", "--hide-scrollbars", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
let socket;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let log = "";
    const timeout = setTimeout(() => reject(new Error("Headless browser startup timed out")), 15000);
    browser.once("error", reject);
    browser.stderr.on("data", (chunk) => {
      log += chunk.toString();
      const match = log.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
    });
  });
  const targets = await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json`)).json();
  const target = targets.find((item) => item.type === "page");
  if (!target) throw new Error("No headless browser page");
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await once(socket, "open");
  let id = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    if (pending.has(message.id)) {
      const { resolve, reject, timer } = pending.get(message.id);
      clearTimeout(timer); pending.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error))); else resolve(message.result);
    }
  });
  function call(method, params = {}) {
    return new Promise((resolve, reject) => {
      const requestId = ++id;
      const timer = setTimeout(() => { pending.delete(requestId); reject(new Error(method + " timed out")); }, 15000);
      pending.set(requestId, { resolve, reject, timer });
      socket.send(JSON.stringify({ id: requestId, method, params }));
    });
  }
  async function evaluate(expression) {
    const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }
  const settle = () => evaluate("new Promise(resolve => setTimeout(resolve, 220))");
  async function check(description, expression) {
    assert.equal(await evaluate(expression), true, description);
    checks.push(description);
  }
  async function clickText(text, selector = "button") {
    await evaluate(`(() => { const node = [...document.querySelectorAll(${JSON.stringify(selector)})].find(node => node.getClientRects().length && node.textContent.trim() === ${JSON.stringify(text)}); if (!node) throw new Error('Missing visible control: ' + ${JSON.stringify(text)}); if(node.disabled) throw new Error('Disabled control: '+${JSON.stringify(text)}); node.click(); })()`);
    await settle();
  }
  async function click(selector) { await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); await settle(); }
  async function input(selector, value) { await evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); node.value = ${JSON.stringify(value)}; node.dispatchEvent(new Event('input', { bubbles: true })); })()`); await settle(); }
  async function select(selector, value) { await evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); node.value = ${JSON.stringify(value)}; node.dispatchEvent(new Event('change', { bubbles: true })); })()`); await settle(); }
  async function submit(selector) { await evaluate(`document.querySelector(${JSON.stringify(selector)}).requestSubmit()`); await settle(); }
  async function scrollTo(selector) { await evaluate(`document.querySelector(${JSON.stringify(selector)}).scrollIntoView({block:'start'})`); await settle(); }
  async function snapshot(name) {
    await settle();
    await evaluate("if(document.activeElement instanceof HTMLElement) document.activeElement.blur()");
    const { data } = await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    await writeFile(path.join(output, name + ".png"), Buffer.from(data, "base64"));
  }
  await call("Page.enable");
  await call("Emulation.setDeviceMetricsOverride", { width: 400, height: 580, deviceScaleFactor: 2, mobile: false });
  let injection;
  async function load(theme, data = fixture, platform = "MacIntel", reduce = false) {
    if (injection) await call("Page.removeScriptToEvaluateOnNewDocument", { identifier: injection });
    const init = await call("Page.addScriptToEvaluateOnNewDocument", { source: `Object.defineProperty(navigator,'platform',{value:${JSON.stringify(platform)}});(${installFakeIPC.toString()})(${JSON.stringify({ ...data, lastRefresh: new Date().toISOString() })});` });
    injection = init.identifier;
    await call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: theme }, { name: "prefers-reduced-motion", value: reduce ? "reduce" : "no-preference" }] });
    await call("Page.navigate", { url });
    for (let attempt = 0; attempt < 60; attempt++) {
      if (await evaluate("!!document.querySelector('main .section-heading, main .empty-state, main [role=alert]')")) { await settle(); return; }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error("Frontend did not initialize");
  }

  for (const theme of ["light", "dark"]) {
    await load(theme);
    await check(theme + ": purpose groups and three canonical tunnels", "document.querySelectorAll('[data-role=remote]').length === 1 && document.querySelectorAll('[data-role=local]').length === 2");
    await check(theme + ": exact forwarding mapping", "document.querySelector('.endpoint').textContent.includes('localhost:2222') && document.querySelector('.endpoint').textContent.includes(':2222')");
    await check(theme + ": inactive local uses Host", "[...document.querySelectorAll('[data-tunnel=\"example-local.test\"] button')].some(n=>n.textContent==='Host') && ![...document.querySelectorAll('[data-role=local] button')].some(n=>n.textContent==='Connect')");
    await check(theme + ": local Host stays in the first viewport", "[...document.querySelectorAll('[data-tunnel=\"example-local.test\"] button')].find(n=>n.textContent==='Host').getBoundingClientRect().bottom < document.querySelector('footer').getBoundingClientRect().top");
    await check(theme + ": entire canonical local card fits above footer", "document.querySelector('[data-tunnel=\"example-local.test\"]').getBoundingClientRect().bottom <= document.querySelector('footer').getBoundingClientRect().top");
    await check(theme + ": no horizontal viewport overflow", "document.querySelector('main').scrollWidth <= 400 && [...document.querySelectorAll('.card-actions')].every(n=>n.scrollWidth<=n.clientWidth)");
    await snapshot("main-" + theme);
    await click('[aria-label="Copy SSH command for port 2222"]');
    await check(theme + ": command and toast match copy", "fake.copied === 'ssh -p 2222 localhost' && document.querySelector('[role=status]').textContent.includes('SSH command copied')");
    await check(theme + ": dock does not overlay footer", "document.querySelector('.notification').getBoundingClientRect().bottom <= document.querySelector('footer').getBoundingClientRect().top");
    await snapshot("toast-" + theme);
    await click('[aria-label="Dismiss notification"]');
    await clickText("Expose");
    await check(theme + ": Expose focuses port and keeps optional name", "document.activeElement === document.querySelector('input[inputmode=numeric]') && !!document.querySelector('input[aria-describedby=name-hint]')");
    await check(theme + ": Advanced starts collapsed", "!document.querySelector('.advanced-options').open");
    await input('input[inputmode="numeric"]', "3000");
    await input('input[aria-describedby="name-hint"]', "example-app");
    await snapshot("expose-" + theme);
    await clickText("Cancel");
    await click('[aria-label="Settings"]');
    await snapshot("settings-" + theme);
    await click('[aria-label="Back to tunnels"]');
    await click('[data-role="remote"] .options');
    await clickText("Logs");
    await check(theme + ": log errors rendered", "document.querySelectorAll('.log-error').length === 1");
    await snapshot("logs-" + theme);
    await click('[aria-label="Back to tunnels"]');
    await evaluate("fake.snapshot.tunnels[1].anonymous=false;fake.publish()");
    await settle();
    await clickText("Make public…");
    await check(theme + ": public change requires confirmation", "!fake.calls.some(c=>c.command==='set_anonymous') && !!document.querySelector('.confirmation')");
    await evaluate("document.querySelector('[data-tunnel=\"example-preview.test\"]').scrollIntoView({block:'start'})");
    await snapshot("public-confirmation-" + theme);
    await clickText("Cancel");
    await evaluate("fake.snapshot.tunnels[1].anonymous=true;fake.publish()");
    await settle();
    await click('[data-tunnel="example-preview.test"] .options');
    await clickText("Delete…");
    await check(theme + ": delete not submitted before confirmation", "!fake.calls.some(c=>c.command==='delete_tunnel') && document.querySelector('.confirmation').textContent.includes('URL stops working')");
    await evaluate("document.querySelector('[data-tunnel=\"example-preview.test\"]').scrollIntoView({block:'start'})");
    await snapshot("delete-confirmation-" + theme);
    await clickText("Cancel");
    await check(theme + ": no browser runtime errors", "fake.errors.length === 0");
  }

  await load("light");
  await evaluate("fake.failClipboard=true");
  await click('[aria-label="Copy SSH command for port 2222"]');
  await check("clipboard failure is explicit and persistent", "document.querySelector('[role=alert]').textContent.includes('Clipboard unavailable') && !![...document.querySelectorAll('.notification button')].find(n=>n.textContent==='Retry')");
  await evaluate("new Promise(resolve=>setTimeout(resolve,2800))");
  await check("failure does not auto-dismiss", "!!document.querySelector('[role=alert]')");
  await evaluate("fake.failClipboard=false");
  await clickText("Retry");
  await check("clipboard retry restores success", "fake.copied==='ssh -p 2222 localhost' && !!document.querySelector('[role=status]')");
  await evaluate("document.querySelector('.notification').dispatchEvent(new MouseEvent('mouseenter'))");
  await evaluate("new Promise(resolve=>setTimeout(resolve,2800))");
  await check("hover pauses success dismissal", "!!document.querySelector('[role=status]')");
  await evaluate("document.querySelector('.notification').dispatchEvent(new MouseEvent('mouseleave'))");
  await click('[aria-label="Copy URL for port 5713"]');
  await check("copy full URL and replace notification", "fake.copied==='https://example-preview-5713.region.devtunnels.example/' && document.querySelectorAll('.notification').length===1");
  await click('[aria-label="Dismiss notification"]');
  await clickText("Expose");
  await input('input[inputmode="numeric"]', "70000");
  await clickText("Expose", ".app-footer button");
  await check("invalid port never reaches IPC", "!fake.calls.some(c=>c.command==='expose_port') && document.querySelector('#port-hint').textContent.includes('between 1 and 65535')");
  await input('input[inputmode="numeric"]', "3000");
  await input('input[aria-describedby="name-hint"]', "example-app");
  await clickText("Expose", ".app-footer button");
  await check("name and private defaults sent to current command", "fake.calls.some(c=>c.command==='expose_port'&&c.args.port===3000&&c.args.name==='example-app'&&c.args.anonymous===false)");
  await click('[aria-label="Dismiss notification"]');
  await click('[data-tunnel="example-preview.test"] .options');
  await clickText("Delete…");
  await evaluate("fake.failCommand='delete_tunnel'");
  await clickText("Delete tunnel");
  await check("failed deletion preserves confirmation", "!!document.querySelector('.confirmation') && !!document.querySelector('[role=alert]') && !document.querySelector('.notification').textContent.includes('Retry')");
  await click('[aria-label="Copy SSH command for port 2222"]');
  await check("unrelated success does not dismiss a persistent error", "document.querySelector('[role=alert]').textContent.includes('Couldn’t delete tunnel')");

  const dual = structuredClone(fixture);
  dual.tunnels[0].hostedElsewhere = false; dual.tunnels[0].hostState = { kind: "connected" };
  dual.tunnels[0].hostedPorts = [{ port: 2222, protocol: "ssh", uri: "", inspectUri: "", listening: true }];
  dual.tunnels[0].state = { kind: "external", pid: 1234 };
  await load("dark", dual, "Win32", true);
  await check("dual-role tunnel appears in both groups", "document.querySelectorAll('[data-tunnel=\"example-ssh.test\"]').length===2");
  await check("external connection exposes Take over / Stop", "[...document.querySelector('[data-role=remote]').querySelectorAll('button')].some(n=>n.textContent==='Take over') && [...document.querySelector('[data-role=remote]').querySelectorAll('button')].some(n=>n.textContent==='Stop')");
  await check("external SSH still supports opening its forwarded port", "!!document.querySelector('[aria-label=\"Open SSH for port 2222\"]')");
  await check("Windows shell and reduced motion applied", "document.querySelector('main').classList.contains('windows') && getComputedStyle(document.querySelector('.tunnel-state')).transitionDuration==='0s'");
  await clickText("Take over");
  await check("external takeover calls current backend", "fake.calls.some(c=>c.command==='adopt_external'&&c.args.id==='example-ssh.test')");
  for (const theme of ["light", "dark"]) { await load(theme, fixture, "Win32"); await snapshot("windows-main-" + theme); }
  const missing = structuredClone(fixture); missing.cliPath = null; missing.tunnels = [];
  await load("light", missing);
  await check("missing CLI has explicit recovery", "document.querySelector('.empty-state').textContent.includes('CLI not found')");
  const signedOut = structuredClone(fixture); signedOut.auth.loggedIn = false; signedOut.tunnels = [];
  await load("dark", signedOut);
  await check("signed out cannot expose", "document.querySelector('.expose-button').disabled && document.querySelector('.empty-state').textContent.includes('Sign in')");
  const empty = structuredClone(fixture); empty.tunnels = [];
  await load("light", empty);
  await check("empty state exposes primary task", "document.querySelector('.empty-state').textContent.includes('No tunnels yet')");
  for (const theme of ["light", "dark"]) {
    await load(theme);
    await clickText("Expose");
    await input('input[inputmode="numeric"]', "3000");
    await input('input[aria-describedby="name-hint"]', "example-app");
    await click(".advanced-options summary");
    await clickText("Dev server preset", "#expose-form button");
    await scrollTo(".header-fields");
    await snapshot("expose-advanced-headers-" + theme);
    await clickText("Add rule");
    await select(".draft-rule select", "org");
    await input(".draft-rule input", "example-org");
    await scrollTo(".draft-rule");
    await snapshot("expose-advanced-access-" + theme);
    await clickText("Expose", ".app-footer button");
    await check(theme + ": Expose wires headers and organization rule", "fake.calls.some(c=>c.command==='expose_port'&&c.args.hostHeader==='unchanged'&&c.args.originHeader==='unchanged'&&c.args.accessRules[0].kind==='org'&&c.args.accessRules[0].value==='example-org')");
    await click('[aria-label="Dismiss notification"]');
    await click('[data-role="remote"] .options');
    await clickText("Manage…");
    await check(theme + ": management shows service lifetime", "document.querySelector('.management-section summary').textContent.includes('30 days')");
    await input('textarea', "Remote development and SSH access");
    await input('input[placeholder="development, preview"]', "development, ssh");
    await snapshot("tunnel-details-" + theme);
    await select('[data-form="about"] select', "7d");
    await submit('[data-form="about"]');
    await check(theme + ": expiration requires non-app confirmation", "document.querySelector('.guard-confirmation').textContent.includes(\"Tunnel Bar didn't create\")&&!fake.calls.find(c=>c.command==='update_tunnel').args.confirm");
    await snapshot("expiration-confirmation-" + theme);
    await clickText("Confirm change");
    await check(theme + ": expiration approval sets only confirm", "fake.calls.some(c=>c.command==='update_tunnel'&&c.args.confirm===true&&c.args.update.expiration==='7d'&&!c.args.allowHostedElsewhere)&&fake.snapshot.tunnels[0].expirationHours===168");
    await click('[aria-label="Dismiss notification"]');
    await input('input[placeholder="development, preview"]', "tunnelbar");
    await submit('[data-form="about"]');
    await check(theme + ": reserved ownership label never reaches IPC", "document.querySelector('[data-form=about] [role=alert]').textContent.includes('reserved')&&!fake.calls.some(c=>c.args.update?.addLabels?.includes('tunnelbar'))");
    await input('input[placeholder="development, preview"]', "development, ssh");
    await click(".management-section[open] > summary");
    await click('[data-section="ports"] > summary');
    await click('[data-port="2222"] > summary');
    await input('[data-form="pin"] input', "2200");
    await submit('[data-form="pin"]');
    await check(theme + ": stable port uses dedicated command", "fake.snapshot.tunnels[0].settings.pinnedPorts['2222']===2200 && fake.calls.some(c=>c.command==='set_pinned_port'&&c.args.remotePort===2222&&c.args.localPort===2200)");
    await click('[aria-label="Dismiss notification"]');
    await scrollTo('[data-form="pin"]');
    await snapshot("ports-and-pins-" + theme);
    await clickText("Dev server preset", '[data-form="port-headers"] button');
    await submit('[data-form="port-headers"]');
    await check(theme + ": header changes require separate remote-host approval", "document.querySelector('.guard-confirmation').textContent.includes('Another machine')&&!fake.calls.find(c=>c.command==='set_port_headers').args.allowHostedElsewhere");
    await snapshot("remote-host-confirmation-" + theme);
    await clickText("Override remote host");
    await check(theme + ": remote-host approval is explicit", "fake.calls.some(c=>c.command==='set_port_headers'&&c.args.allowHostedElsewhere===true&&c.args.headers.hostHeader==='unchanged')");
    await click('[aria-label="Dismiss notification"]');
    await scrollTo('[data-form="port-headers"]');
    await snapshot("port-headers-" + theme);
    await input('[data-form="host-map"] input', "22");
    await submit('[data-form="host-map"]');
    await check(theme + ": host mapping uses the local-only mapping command", "fake.calls.some(c=>c.command==='set_port_map'&&c.args.remotePort===2222&&c.args.localPort===22)&&fake.snapshot.tunnels[0].settings.hostPortMap['2222']===22");
    await click('[aria-label="Dismiss notification"]');
    await scrollTo('[data-form="host-map"]');
    await snapshot("host-port-mapping-" + theme);
    await click('[aria-label="Back to tunnels"]');
    await check(theme + ": pinned address is displayed", "document.querySelector('[data-role=remote] .address').textContent==='localhost:2200'");
    await click('[data-role=remote] .address');
    await check(theme + ": pinned address is copied", "fake.copied==='localhost:2200'");
    await click('[aria-label="Copy SSH command for port 2222"]');
    await check(theme + ": Copy SSH keeps backend contract and uses pin", "fake.copied==='ssh -p 2200 localhost'&&fake.calls.find(c=>c.command==='ssh_command').args.localPort===2222");
    await click('[aria-label="Dismiss notification"]');
    await click('[data-tunnel="example-preview.test"] .options');
    await clickText("Manage…");
    await click(".management-section[open] > summary");
    await click('[data-section="access"] > summary');
    await check(theme + ": stored access rules show anonymous access", "document.querySelector('.access-entry').textContent.includes('Allow · Anyone')");
    await select('[data-section="access"] > .field select', "5713");
    await check(theme + ": inherited rules cannot be removed", "document.querySelector('.access-entry').textContent.includes('Inherited from tunnel')&&!document.querySelector('.access-entry button')");
    await select('[data-form="add-rule"] select', "org");
    await input('[data-form="add-rule"] input', "example-org");
    await submit('[data-form="add-rule"]');
    await check(theme + ": port-level rule retains correct scope", "fake.calls.some(c=>c.command==='add_access_rule'&&c.args.rule.port===5713&&c.args.rule.value==='example-org')");
    await click('[aria-label="Dismiss notification"]');
    await scrollTo('[data-section="access"]');
    await snapshot("access-rules-" + theme);
    await clickText("Remove…", ".access-entry button");
    await clickText("Confirm");
    await check(theme + ": own rule removal retains backend index", "fake.calls.some(c=>c.command==='remove_access_rule'&&c.args.port===5713&&c.args.index===0)");
    await click('[aria-label="Dismiss notification"]');
    await click('[data-section="access"] > summary');
    await click('[data-section="headers"] > summary');
    await clickText("Dev server preset", '[data-form="tunnel-headers"] button');
    await scrollTo('[data-section="headers"]');
    await snapshot("tunnel-headers-" + theme);
    await submit('[data-form="tunnel-headers"]');
    await check(theme + ": tunnel header defaults use update command", "fake.calls.some(c=>c.command==='update_tunnel'&&c.args.update.hostHeader==='unchanged')");
    await click('[aria-label="Dismiss notification"]');
    await click('[aria-label="Back to tunnels"]');
    await click('[aria-label="Settings"]');
    await input('input[placeholder="CmdOrCtrl+Alt+Shift+T"]', "CmdOrCtrl+Alt+K");
    await clickText("Save shortcut");
    await check(theme + ": hotkey persisted by dedicated command", "fake.snapshot.settings.hotkey==='CmdOrCtrl+Alt+K'");
    await click('[aria-label="Dismiss notification"]');
    await clickText("preview", ".label-chips button");
    await check(theme + ": label filter uses dedicated command", "fake.snapshot.settings.labelFilter[0]==='preview'&&fake.snapshot.tunnels.every(t=>t.matchesFilter===false)");
    await click('[aria-label="Dismiss notification"]');
    await scrollTo(".label-chips");
    await snapshot("settings-label-filter-" + theme);
    await click('[aria-label="Back to tunnels"]');
    await check(theme + ": filtered empty state is explicit", "document.querySelector('.empty-state').textContent.includes('No matching tunnels')");
    await clickText("Clear label filter");
    await click('[aria-label="Dismiss notification"]');
    await click('[aria-label="Settings"]');
    await scrollTo(".login-panel");
    await snapshot("settings-account-" + theme);
    await click(".login-mode input");
    await clickText("GitHub", ".login-panel button");
    await check(theme + ": device-code sign-in uses provider and mode", "fake.calls.some(c=>c.command==='start_login'&&c.args.provider==='github'&&c.args.deviceCode===true)&&document.querySelector('.device-code').textContent.includes('ABCD-EFGH')");
    await scrollTo(".login-panel");
    await snapshot("sign-in-device-code-" + theme);
    await clickText("Copy code");
    await check(theme + ": device code copied exactly", "fake.copied==='ABCD-EFGH'");
    await click('[aria-label="Dismiss notification"]');
    await clickText("Cancel sign-in");
    await check(theme + ": sign-in cancellation is wired", "fake.calls.some(c=>c.command==='cancel_login')&&fake.snapshot.login===null");
    await clickText("Sign out…");
    await check(theme + ": sign-out requires inline approval", "!!document.querySelector('.login-panel .confirmation')&&!fake.calls.some(c=>c.command==='logout')");
    await scrollTo(".login-panel");
    await snapshot("sign-out-confirmation-" + theme);
    await clickText("Sign out");
    await check(theme + ": sign-out uses backend command", "fake.calls.some(c=>c.command==='logout')&&!fake.snapshot.auth.loggedIn");
    await check(theme + ": Phase 3 has no runtime errors", "fake.errors.length===0");
  }
  await load("light");
  await click('[data-role="remote"] .options');
  await clickText("Manage…");
  await click(".management-section[open] > summary");
  await click('[data-section="ports"] > summary');
  await input('[data-form="add-port"] input', "8080");
  await submit('[data-form="add-port"]');
  await check("adding a remote-host port never overrides by default", "document.querySelector('.guard-confirmation').textContent.includes('Another machine')&&!fake.calls.find(c=>c.command==='add_port').args.allowHostedElsewhere");
  await clickText("Override remote host");
  await check("confirmed port addition reaches current backend", "fake.snapshot.tunnels[0].ports.some(p=>p.portNumber===8080)");
  await click('[aria-label="Dismiss notification"]');
  await click('[data-port="8080"] > summary');
  await clickText("Remove port…", '[data-port="8080"] button');
  await clickText("Confirm");
  await check("port removal first checks remote host", "document.querySelector('.guard-confirmation').textContent.includes('Another machine')");
  await clickText("Override remote host");
  await check("remote-host override does not implicitly confirm ownership", "document.querySelector('.guard-confirmation').textContent.includes(\"Tunnel Bar didn't create\")&&fake.snapshot.tunnels[0].ports.some(p=>p.portNumber===8080)");
  await clickText("Confirm change");
  await check("both port-removal guards require separate approvals", "fake.calls.some(c=>c.command==='remove_port'&&c.args.allowHostedElsewhere===true&&c.args.confirm===true)&&!fake.snapshot.tunnels[0].ports.some(p=>p.portNumber===8080)");
  await click('[aria-label="Dismiss notification"]');
  await clickText("Delete tunnel…");
  await clickText("Confirm");
  await clickText("Override remote host");
  await check("delete remains pending after only remote-host approval", "fake.snapshot.tunnels.some(t=>t.id==='example-ssh.test')&&document.querySelector('.guard-confirmation').textContent.includes(\"Tunnel Bar didn't create\")");
  await clickText("Confirm change");
  await check("both deletion approvals are explicit and return to list", "!fake.snapshot.tunnels.some(t=>t.id==='example-ssh.test')&&document.querySelector('h1').textContent==='Tunnel Bar'");
  for (const theme of ["light", "dark"]) {
    await load(theme);
    await clickText("Expose");
    await input('input[placeholder="3000"]', "22");
    await input('input[placeholder="Same as local"]', "70000");
    await clickText("Expose", ".app-footer button");
    await check(theme + ": invalid optional tunnel port is rejected before IPC", "!fake.calls.some(c=>c.command==='expose_port')&&document.activeElement.placeholder==='Same as local'");
    await input('input[placeholder="Same as local"]', "2222");
    await input('input[aria-describedby="name-hint"]', "example-ssh");
    await select("#expose-form > .field select", "auto");
    await snapshot("expose-port-mapping-" + theme);
    await clickText("Expose", ".app-footer button");
    await check(theme + ": Expose sends distinct server and tunnel ports", "fake.calls.some(c=>c.command==='expose_port'&&c.args.port===22&&c.args.remotePort===2222&&c.args.protocol==='auto')");
    const mapped = structuredClone(fixture);
    mapped.tunnels[2].hostState = {kind:"connected"};
    mapped.tunnels[2].summary.hostConnections = 1;
    mapped.tunnels[2].ports = [{portNumber:2222,protocol:"auto",localPort:22,portUri:""}];
    mapped.tunnels[2].hostedPorts = [{port:2222,localPort:22,protocol:"auto",listening:false,uri:"",inspectUri:""}];
    mapped.tunnels[2].settings.hostPortMap = {"2222":22};
    mapped.tunnels[2].portMaps = [{remotePort:2222,localPort:22,listening:true,error:null}];
    await load(theme,mapped);
    await check(theme + ": hosted mapping and warning use the local server", "document.querySelector('[data-tunnel=\"example-local.test\"] .endpoint').textContent.includes('localhost:22')&&document.querySelector('[data-tunnel=\"example-local.test\"] .inline-warning').textContent.includes('localhost:22')&&!document.querySelector('[data-tunnel=\"example-local.test\"] .inline-warning').textContent.includes('localhost:2222')");
    await scrollTo('[data-tunnel="example-local.test"]');
    await snapshot("hosting-port-mapping-" + theme);
  }
  await load("light");
  await click('[data-tunnel="example-local.test"] .options');
  await clickText("Manage…");
  await click(".management-section[open] > summary");
  await click('[data-section="ports"] > summary');
  await input('[data-form="add-port"] input[placeholder="3000"]', "22");
  await input('[data-form="add-port"] input[placeholder="Same as local"]', "2222");
  await submit('[data-form="add-port"]');
  await check("mapped port addition advertises the requested tunnel port", "fake.snapshot.tunnels[2].ports.some(p=>p.portNumber===2222&&p.localPort===22)&&fake.calls.some(c=>c.command==='add_port'&&c.args.port===22&&c.args.remotePort===2222)");
  await click('[aria-label="Dismiss notification"]');
  await click('[aria-label="Back to tunnels"]');
  await click('[data-tunnel="example-local.test"] .auto-toggle input');
  await check("automation changes preserve host maps and pin settings", "fake.calls.some(c=>c.command==='update_tunnel_settings'&&c.args.settings.hostPortMap['2222']===22&&c.args.settings.pinnedPorts)");
  const race = structuredClone(fixture); race.simulateSnapshotRace = true;
  await load("light",race);
  await check("initial snapshot cannot overwrite a newer event", "document.querySelector('.tunnel-name').textContent==='Latest snapshot'");
  const failingInitial = structuredClone(fixture); failingInitial.failOnStart = "get_snapshot";
  await load("dark", failingInitial);
  await check("initial snapshot failure is actionable", "document.querySelector('[role=alert]').textContent.includes('Couldn’t load tunnels')");
  await evaluate("fake.failCommand=''");
  await clickText("Retry");
  await check("initial snapshot retry recovers without a stale error", "document.querySelectorAll('.tunnel-card').length===3&&!document.querySelector('[role=alert]')");
  await click('[aria-label="Copy SSH command for port 2222"]');
  await evaluate("document.querySelector('[aria-label=\"Dismiss notification\"]').focus()");
  await evaluate("new Promise(resolve=>setTimeout(resolve,2800))");
  await check("keyboard focus pauses toast dismissal", "!!document.querySelector('[role=status]')");
  await click('[aria-label="Dismiss notification"]');
  const externalHost = structuredClone(fixture); externalHost.tunnels[1].hostState={kind:"external",pid:4321};
  await load("light",externalHost);
  await check("external host ownership is not taken over or stopped", "!document.querySelector('[data-tunnel=\"example-preview.test\"] .card-actions button')");
  const protectedHost = structuredClone(fixture); protectedHost.tunnels[1].createdByApp=false;
  await load("light",protectedHost);
  await clickText("Require sign-in");
  await check("removing public access on a non-app tunnel is confirmed inline", "!!document.querySelector('[data-role=local] .confirmation')&&fake.snapshot.tunnels[1].anonymous");
  await clickText("Confirm change");
  await check("confirmed private access preserves the false allow argument", "!fake.snapshot.tunnels[1].anonymous&&fake.calls.some(c=>c.command==='set_anonymous'&&c.args.allow===false&&c.args.confirm===true)");
  await load("light");
  await click('[aria-label="Settings"]');
  await input('input[placeholder="CmdOrCtrl+Alt+Shift+T"]', "K");
  await clickText("Save shortcut");
  await check("invalid shortcut leaves the persisted setting intact", "fake.snapshot.settings.hotkey==='CmdOrCtrl+Alt+Shift+T'&&document.querySelector('[role=alert]').textContent.includes('modifier')");
  console.log(`${checks.length} headless UI assertions passed; screenshots saved to ${output}`);
  await writeFile(path.join(output, "validation.json"), JSON.stringify({ checks, synthetic: true }, null, 2) + "\n");
} catch (error) {
  console.error(`Failed after ${checks.length} assertions; last completed: ${checks.at(-1) ?? "initialization"}`);
  throw error;
} finally {
  socket?.close();
  browser.kill("SIGTERM");
  if (browser.exitCode === null) await once(browser, "close");
  server.close();
  await rm(profile, { recursive: true, force: true });
}
