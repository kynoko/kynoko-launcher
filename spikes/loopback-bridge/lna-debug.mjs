// Why does Edge refuse even with the permission granted? Capture the network log.
import { spawn, execSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const here = fileURLToPath(new URL('.', import.meta.url));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const page = readFileSync(join(here, 'page', 'open.html'));
const kids = [];
const kill = (c) => { try { execSync(`taskkill /T /F /PID ${c.pid}`, { stdio: 'ignore' }); } catch {} };
const srv = http.createServer((q, s) => { if (q.method === 'POST') { q.resume(); return s.end('ok'); } s.setHeader('Content-Type','text/html'); s.end(page); });
await new Promise((r) => srv.listen(0, '127.0.0.1', r));
const pport = srv.address().port, origin = `http://127.0.0.1:${pport}`;
const dir = mkdtempSync(join(tmpdir(), 'dbg-')); const file = join(dir, 'f.bin'); writeFileSync(file, 'hello');
const br = spawn(join(here, 'target/release/loopback-bridge.exe'), ['--file', file, '--origin', origin]); kids.push(br);
const frag = JSON.parse(String(await new Promise((r) => br.stdout.once('data', r)))).fragment;
const profile = mkdtempSync(join(tmpdir(), 'dbgp-'));
const grant = process.argv[2] === 'grant';
const edge = spawn('C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', ['--headless=new', `--user-data-dir=${profile}`, '--no-first-run', `--ip-address-space-overrides=127.0.0.1:${pport}=public`, '--remote-debugging-port=0', 'about:blank'], { stdio: 'ignore' }); kids.push(edge);
let pf; while (!pf) { await sleep(200); try { pf = readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n'); } catch {} }
const ws = new WebSocket(`ws://127.0.0.1:${pf[0]}${pf[1]}`); await new Promise((r) => ws.addEventListener('open', r, { once: true }));
let id = 0; const pending = new Map();
ws.addEventListener('message', (ev) => { const m = JSON.parse(ev.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } else if (/loadingFailed|requestWillBeSentExtraInfo|responseReceivedExtraInfo|issueAdded|Log.entryAdded|consoleAPI/.test(m.method)) console.log(m.method, JSON.stringify(m.params).slice(0, 700)); });
const call = (method, params = {}, sessionId) => new Promise((r) => { const my = ++id; pending.set(my, r); ws.send(JSON.stringify({ id: my, method, params, sessionId })); });
if (grant) console.log('grant', JSON.stringify(await call('Browser.grantPermissions', { origin, permissions: ['localNetworkAccess'] })));
const { result: { targetId } } = await call('Target.createTarget', { url: 'about:blank' });
const { result: { sessionId } } = await call('Target.attachToTarget', { targetId, flatten: true });
await call('Network.enable', {}, sessionId); await call('Audits.enable', {}, sessionId); await call('Log.enable', {}, sessionId); await call('Runtime.enable', {}, sessionId);
await call('Page.navigate', { url: `${origin}/open.html#${frag}` }, sessionId);
await sleep(8000);
ws.close(); kids.forEach(kill); srv.close(); await sleep(1000); try { rmSync(dir, { recursive: true, force: true }); rmSync(profile, { recursive: true, force: true }); } catch {}
process.exit(0);
