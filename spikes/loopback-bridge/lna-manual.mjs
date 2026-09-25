// Phase 0, part 3: the Local Network Access prompt, seen by a human.
//
// Opens a VISIBLE browser twice on the same throwaway profile and the same
// "public" page origin:
//   session 1: the prompt should appear - answer "Allow" (and tick
//              "remember" if the browser offers it);
//   session 2: the browser is restarted - does it still remember?
// Each session closes by itself once the page has reported. Nothing is left
// behind (profile, test file and processes are removed at the end).
//
// Usage: node lna-manual.mjs [firefox|edge]
// Result: printed, and written to manual-results.json

import { spawn, execSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const which = process.argv[2] || 'firefox';
const page = readFileSync(join(here, 'page', 'open.html'));
const PROMPT_WAIT_MS = 5 * 60_000;

const children = [];
const kill = (c) => { try { execSync(`taskkill /T /F /PID ${c.pid}`, { stdio: 'ignore' }); } catch { /* gone */ } };
process.on('exit', () => children.forEach(kill));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ONE page server for both sessions: the permission is keyed by origin, port included.
let deliver;
const server = http.createServer((req, res) => {
  if (req.method === 'POST' && req.url === '/results') {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => { res.end('ok'); deliver?.(JSON.parse(body)); });
    return;
  }
  res.setHeader('Content-Type', 'text/html; charset=utf-8');
  res.end(page);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;
const origin = `http://127.0.0.1:${pagePort}`;

const dir = mkdtempSync(join(tmpdir(), 'lna-manual-'));
const file = join(dir, 'sample.bin');
writeFileSync(file, randomBytes(1024 * 1024));
const profile = mkdtempSync(join(tmpdir(), `lna-manual-profile-${which}-`));

if (which === 'firefox') {
  const prefs = {
    'network.lna.address_space.public.override': `127.0.0.1:${pagePort}`,
    'browser.aboutwelcome.enabled': false,
    'browser.shell.checkDefaultBrowser': false,
    'browser.startup.homepage_override.mstone': 'ignore',
    'datareporting.policy.dataSubmissionPolicyBypassNotification': true,
  };
  writeFileSync(join(profile, 'user.js'),
    Object.entries(prefs).map(([k, v]) => `user_pref(${JSON.stringify(k)}, ${JSON.stringify(v)});`).join('\n'));
}

function launch(url) {
  const [exe, args] = which === 'firefox'
    ? ['C:/Program Files/Mozilla Firefox/firefox.exe', ['-no-remote', '-profile', profile, url]]
    : ['C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
       [`--user-data-dir=${profile}`, '--no-first-run', `--ip-address-space-overrides=127.0.0.1:${pagePort}=public`, url]];
  const proc = spawn(exe, args, { stdio: 'ignore' });
  children.push(proc);
  return proc;
}

async function session(n) {
  const bridge = spawn(join(here, 'target', 'release', 'loopback-bridge.exe'),
    ['--file', file, '--origin', origin, '--idle-secs', String(PROMPT_WAIT_MS / 1000 + 60)]);
  children.push(bridge);
  const { fragment } = JSON.parse(String(await new Promise((r) => bridge.stdout.once('data', r))));
  const result = new Promise((r) => (deliver = r));
  console.log(`\n=== Session ${n}: ${which} opens now.${n === 1 ? ' Answer "Allow" to the prompt.' : ' Do NOT click anything (20 s).'}`);
  // Session 2 must not wait on a human: a prompt shown again simply times out.
  const timeout = n === 1 ? PROMPT_WAIT_MS : 20_000;
  const proc = launch(`${origin}/open.html#${fragment}&timeout=${timeout}`);
  const r = await Promise.race([result, sleep(PROMPT_WAIT_MS + 30_000).then(() => null)]);
  kill(proc);
  kill(bridge);
  await sleep(2000); // let the profile flush before the next launch
  if (!r) return { session: n, outcome: 'no report' };
  const s = r.steps;
  return {
    session: n,
    allOk: Object.values(s).every((v) => v.ok),
    // A first request that took seconds waited on a human; milliseconds means no prompt.
    firstRequestMs: s.meta?.ms,
    firstRequest: s.meta?.ok ? 'ok' : s.meta?.error,
    writeBack: s.write?.ok && s.rereadMatchesWrite?.value === true,
  };
}

const results = { browser: which, sessions: [await session(1), await session(2)] };
const [a, b] = results.sessions;
results.verdict =
  !a.allOk ? 'session 1 failed (prompt refused or not shown?)'
  : b.allOk ? 'remembered across a restart'
  : 'NOT remembered: session 2 prompted again (or refused) without an answer';
console.log(JSON.stringify(results, null, 1));
writeFileSync(join(here, 'manual-results.json'), JSON.stringify(results, null, 1));

server.close();
children.forEach(kill);
await sleep(1500);
for (const d of [dir, profile]) { try { rmSync(d, { recursive: true, force: true }); } catch { /* locked */ } }
process.exit(0);
