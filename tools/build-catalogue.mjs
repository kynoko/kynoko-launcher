#!/usr/bin/env node
/**
 * Builds the catalogue snapshot bundled in Kynoko Launcher
 * (app/src-tauri/catalogue.json), in the format of docs/SPEC.md section 4.
 *
 * Until the platform publishes the catalogue itself, it is assembled here
 * from what each app already publishes about itself: its
 * `assets/kynoko-app.json` (tiles, and the files each tile opens).
 *
 * Usage:
 *   node tools/build-catalogue.mjs apps.json
 *
 * apps.json lists the apps to include:
 *   [{ "code": "Office", "url": "https://office.kynoko.com/",
 *      "names": { "en": "Kynoko Office", "fr": "Kynoko Office" },
 *      "manifest": "<URL or local path of its kynoko-app.json>" }]
 *
 * `url` is where the launcher sends people (the production address);
 * `manifest` is only where this script reads the tiles from, and may point
 * at a local checkout. Nothing about development environments is written
 * into the output.
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const listPath = process.argv[2];
if (!listPath) {
  console.error('usage: node tools/build-catalogue.mjs apps.json');
  process.exit(2);
}

const apps = JSON.parse(readFileSync(listPath, 'utf8'));
const out = { v: 1, generatedAt: new Date().toISOString(), apps: [] };

for (const app of apps) {
  const manifest = JSON.parse(
    /^https?:/.test(app.manifest) ? await (await fetch(app.manifest)).text() : readFileSync(app.manifest, 'utf8'),
  );
  if (manifest.appCode !== app.code) {
    throw new Error(`build-catalogue: ${app.manifest} is ${manifest.appCode}, not ${app.code}`);
  }
  const facades = (manifest.tiles ?? [])
    .filter((tile) => Array.isArray(tile.files) && tile.files.length)
    .map((tile) => ({ path: tile.path, names: tile.names ?? {}, files: tile.files }));
  out.apps.push({
    code: app.code,
    url: new URL(app.url).href,
    names: app.names,
    status: 'live',
    facades,
  });
  console.log(`${app.code}: ${facades.length} facade(s), ${facades.reduce((n, f) => n + f.files.length, 0)} file type(s)`);
}

const target = join(here, '..', 'app', 'src-tauri', 'catalogue.json');
writeFileSync(target, JSON.stringify(out, null, 2) + '\n');
console.log(`catalogue -> ${target}`);
