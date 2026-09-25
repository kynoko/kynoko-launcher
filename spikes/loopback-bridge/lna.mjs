// Phase 0, part 2: Local Network Access.
//
// A real app lives on a PUBLIC https origin, and browsers now gate requests
// from a public page to 127.0.0.1. Here the page is served locally but each
// browser is told, through its own testing switch, that the page's address is
// public:
//   Chromium: --ip-address-space-overrides=127.0.0.1:<port>=public
//   Firefox:  network.lna.address_space.public.override = 127.0.0.1:<port>
//
// Cases:
//   edge-default      public page, nobody answers the prompt
//   edge-granted      same, the `loopbackNetwork` permission granted through CDP
//                     beforehand (Chromium splits it from `localNetwork`)
//   firefox-default   public page, Firefox defaults otherwise
//   firefox-blocking  same, with network.lna.blocking = true (what strict ETP sets)
//   firefox-off       same, with network.lna.blocking = false: proves LNA is the gate
//
// Usage: node lna.mjs

import { spawn, execSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const bridgeExe = join(here, 'target', 'release', 'loopback-bridge.exe');
const page = readFileSync(join(here, 'page', 'open.html'));
const EDGE = 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const FIREFOX = 'C:/Program Files/Mozilla Firefox/firefox.exe';

const children = [];
const kill = (c) => { try { execSync(`taskkill /T /F /PID ${c.pid}`, { stdio: 'ignore' }); } catch { /* gone */ } };
process.on('exit', () => children.forEach(kill));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function pageServer() {
  return new Promise((resolve) => {
    let deliver;
    const result = new Promise((r) => (deliver = r));
    const server = http.createServer((req, res) => {
      if (req.method === 'POST' && req.url === '/results') {
        let body = '';
        req.on('data', (c) => (body += c));
        req.on('end', () => { res.end('ok'); deliver(JSON.parse(body)); });
        return;
      }
      res.setHeader('Content-Type', 'text/html; charset=utf-8');
      res.end(page);
    });
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port, result }));
  });
}

function startBridge(file, origin) {
  return new Promise((resolve, reject) => {
    const child = spawn(bridgeExe, ['--file', file, '--origin', origin, '--idle-secs', '90']);
    children.push(child);
    child.stdout.once('data', (d) => resolve({ child, ...JSON.parse(String(d)) }));
    child.once('error', reject);
  });
}

/** Opens `url` in Edge; with `grant`, pre-grants Local Network Access over CDP. */
async function launchEdge(profile, pagePort, url, grant) {
  const args = [
    '--headless=new', `--user-data-dir=${profile}`, '--no-first-run', '--disable-gpu',
    `--ip-address-space-overrides=127.0.0.1:${pagePort}=public`,
    '--remote-debugging-port=0', 'about:blank',
  ];
  const proc = spawn(EDGE, args, { stdio: 'ignore' });
  children.push(proc);
  let portFile;
  for (let i = 0; i < 50 && !portFile; i++) {
    await sleep(200);
    try { portFile = readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n'); } catch { /* not yet */ }
  }
  const ws = new WebSocket(`ws://127.0.0.1:${portFile[0]}${portFile[1]}`);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const call = (method, params = {}) => new Promise((resolve) => {
    const my = ++id;
    const onMsg = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id === my) { ws.removeEventListener('message', onMsg); resolve(m); }
    };
    ws.addEventListener('message', onMsg);
    ws.send(JSON.stringify({ id: my, method, params }));
  });
  let grantResult = null;
  if (grant) {
    const origin = new URL(url).origin;
    grantResult = await call('Browser.grantPermissions', { origin, permissions: ['loopbackNetwork'] });
  }
  await call('Target.createTarget', { url });
  return { proc, grantResult, close: () => ws.close() };
}

async function launchFirefox(profile, pagePort, url, prefs) {
  const all = { 'network.lna.address_space.public.override': `127.0.0.1:${pagePort}`, ...prefs };
  writeFileSync(join(profile, 'user.js'),
    Object.entries(all).map(([k, v]) => `user_pref(${JSON.stringify(k)}, ${JSON.stringify(v)});`).join('\n'));
  const proc = spawn(FIREFOX, ['--headless', '-no-remote', '-profile', profile, url], { stdio: 'ignore' });
  children.push(proc);
  return { proc, close: () => {} };
}

async function runCase(name, launch) {
  const dir = mkdtempSync(join(tmpdir(), `lna-${name}-`));
  const file = join(dir, 'sample.bin');
  writeFileSync(file, randomBytes(1024 * 1024));
  const pg = await pageServer();
  const origin = `http://127.0.0.1:${pg.port}`;
  const bridge = await startBridge(file, origin);
  const profile = mkdtempSync(join(tmpdir(), `lna-profile-${name}-`));
  const b = await launch(profile, pg.port, `${origin}/open.html#${bridge.fragment}`);
  let out;
  try {
    const r = await Promise.race([pg.result, sleep(120_000).then(() => null)]);
    out = r
      ? Object.fromEntries(Object.entries(r.steps).map(([k, v]) => [k, v.ok ? 'ok' : v.error]))
      : 'no report within 120 s';
  } finally {
    b.close();
    kill(b.proc);
    kill(bridge.child);
    pg.server.close();
  }
  await sleep(1500);
  for (const d of [dir, profile]) { try { rmSync(d, { recursive: true, force: true }); } catch { /* locked */ } }
  return { case: name, grant: b.grantResult ?? undefined, result: out, leftovers: existsSync(dir) };
}

const only = process.argv[2];
const cases = [
  ['edge-default', (p, port, url) => launchEdge(p, port, url, false)],
  ['edge-granted', (p, port, url) => launchEdge(p, port, url, true)],
  ['firefox-default', (p, port, url) => launchFirefox(p, port, url, {})],
  ['firefox-blocking', (p, port, url) => launchFirefox(p, port, url, { 'network.lna.blocking': true })],
  ['firefox-off', (p, port, url) => launchFirefox(p, port, url, { 'network.lna.blocking': false })],
];
for (const [name, launch] of cases.filter(([n]) => !only || n.startsWith(only))) {
  console.log(JSON.stringify(await runCase(name, launch), null, 1));
}
process.exit(0);
