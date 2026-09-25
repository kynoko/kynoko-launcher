// Phase 0 harness: runs the bridge against real browsers, headless.
//
// For each browser: a random test file, a bridge allowed for origin A, the
// page loaded from origin A (must read + write back) and from origin B (must
// be refused), plus raw HTTP probes for the Host/Origin/token guards.
// Every process it starts is killed before it exits.
//
// Usage: node harness.mjs [sizeMB=64]

import { spawn, execSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const bridgeExe = join(here, 'target', 'release', 'loopback-bridge.exe');
const page = readFileSync(join(here, 'page', 'open.html'));
const sizeMB = Number(process.argv[2] || 64);

const BROWSERS = [
  {
    name: 'edge',
    exe: 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    args: (profile, url) => ['--headless=new', `--user-data-dir=${profile}`, '--no-first-run', '--disable-gpu', url],
  },
  {
    name: 'firefox',
    exe: 'C:/Program Files/Mozilla Firefox/firefox.exe',
    args: (profile, url) => ['--headless', '-no-remote', '-profile', profile, url],
  },
];

const children = [];
const killAll = () => {
  for (const c of children) {
    try { execSync(`taskkill /T /F /PID ${c.pid}`, { stdio: 'ignore' }); } catch { /* already gone */ }
  }
};
process.on('exit', killAll);

const sha = (buf) => createHash('sha256').update(buf).digest('hex');

/** Serves the page and collects the one POST /results it sends. */
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
    server.listen(0, 'localhost', () => resolve({ server, port: server.address().port, result }));
  });
}

function startBridge(file, origin) {
  return new Promise((resolve, reject) => {
    const child = spawn(bridgeExe, ['--file', file, '--origin', origin, '--idle-secs', '60']);
    children.push(child);
    child.stderr.on('data', (d) => process.stderr.write(`[bridge] ${d}`));
    child.stdout.once('data', (d) => resolve({ child, ...JSON.parse(String(d)) }));
    child.once('error', reject);
  });
}

/** Raw probe: lets us forge Host and Origin, which a browser never would. */
function probe(port, path, headers, method = 'GET') {
  return new Promise((resolve) => {
    const req = http.request({ host: '127.0.0.1', port, path, method, headers }, (res) => {
      res.resume();
      res.on('end', () => resolve(res.statusCode));
    });
    req.on('error', (e) => resolve(`error ${e.code}`));
    req.end();
  });
}

function withTimeout(promise, ms, label) {
  return Promise.race([promise, new Promise((_, rej) => setTimeout(() => rej(new Error(`${label}: timeout`)), ms))]);
}

async function runBrowser(browser) {
  const dir = mkdtempSync(join(tmpdir(), `bridge-${browser.name}-`));
  const file = join(dir, `sample file é.bin`); // space + accent on purpose
  const original = randomBytes(sizeMB * 1024 * 1024);
  writeFileSync(file, original);

  const a = await pageServer();
  const b = await pageServer();
  const originA = `http://localhost:${a.port}`;
  const report = { browser: browser.name, checks: {} };
  const check = (name, ok, detail) => (report.checks[name] = { ok, detail });

  // 1. Guards, probed raw.
  const g = await startBridge(file, originA);
  const p = `/s/${g.token}/meta`;
  check('guard: right host+origin', (await probe(g.port, p, { Host: `127.0.0.1:${g.port}`, Origin: originA })) === 200);
  check('guard: rebound host refused', (await probe(g.port, p, { Host: `evil.example:${g.port}`, Origin: originA })) === 421);
  check('guard: foreign origin refused', (await probe(g.port, p, { Host: `127.0.0.1:${g.port}`, Origin: 'https://evil.example' })) === 403);
  check('guard: no origin refused', (await probe(g.port, p, { Host: `127.0.0.1:${g.port}` })) === 403);
  check('guard: wrong token', (await probe(g.port, `/s/${'0'.repeat(64)}/meta`, { Host: `127.0.0.1:${g.port}`, Origin: originA })) === 404);

  // 2. The allowed page reads and writes back.
  const profile = mkdtempSync(join(tmpdir(), `profile-${browser.name}-`));
  const proc = spawn(browser.exe, browser.args(profile, `${originA}/open.html#${g.fragment}`), { stdio: 'ignore' });
  children.push(proc);
  try {
    const r = await withTimeout(a.result, 90_000, 'allowed page');
    report.page = r.steps;
    report.ua = r.ua;
    check('page: read intact', r.steps.read?.value?.sha256 === sha(original), `${r.steps.read?.ms} ms for ${sizeMB} MB`);
    check('page: write accepted', r.steps.write?.ok && r.steps.write.value.changed);
    check('disk: holds the written bytes', sha(readFileSync(file)) === r.steps.write?.value?.sha256);
    check('page: stale write refused (412)', r.steps.staleWriteRefused?.value === 412);
    check('page: reread matches', r.steps.rereadMatchesWrite?.value === true);
    check('page: close', r.steps.close?.value === 204);
  } catch (e) {
    check('page: allowed flow', false, String(e));
  }
  try { execSync(`taskkill /T /F /PID ${proc.pid}`, { stdio: 'ignore' }); } catch { /* gone */ }

  // 3. A page from ANOTHER origin must not get anything.
  const g2 = await startBridge(file, originA);
  const profile2 = mkdtempSync(join(tmpdir(), `profile-${browser.name}-`));
  const proc2 = spawn(browser.exe, browser.args(profile2, `http://localhost:${b.port}/open.html#${g2.fragment}&mode=foreign`), { stdio: 'ignore' });
  children.push(proc2);
  try {
    const r = await withTimeout(b.result, 60_000, 'foreign page');
    report.foreign = r.steps;
    check('foreign page: meta blocked', r.steps.foreignMeta?.ok === false, r.steps.foreignMeta?.error);
    check('foreign page: content blocked', r.steps.foreignContent?.ok === false, r.steps.foreignContent?.error);
  } catch (e) {
    check('foreign page: flow', false, String(e));
  }
  try { execSync(`taskkill /T /F /PID ${proc2.pid}`, { stdio: 'ignore' }); } catch { /* gone */ }

  a.server.close();
  b.server.close();
  killAll();
  await new Promise((r) => setTimeout(r, 1500));
  for (const d of [dir, profile, profile2]) {
    try { rmSync(d, { recursive: true, force: true }); } catch { /* locked by a dying process */ }
  }
  const leftovers = existsSync(dir) ? 'dir left behind' : 'clean';
  return { ...report, tmp: leftovers };
}

for (const b of BROWSERS) {
  const r = await runBrowser(b);
  console.log(JSON.stringify(r, null, 1));
}
killAll();
process.exit(0);
