import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ICONS, el, icon, keyed } from './dom';
import { Lang, pickLang, t } from './i18n';
import { confirmDialog, toast, tooltip } from './overlay';
import { titleBar, windowIconImg } from './titlebar';

interface Profile { id: string; name: string; default: boolean }
interface Browser { id: string; name: string; engine: string; webEngine: string; profiles: Profile[] }
interface TypeGroup { facade: string; exts: { ext: string; on: boolean }[] }
interface ShortcutItem { key: string; name: string; on: boolean }
/** `browsersOnly`: what the Kynoko window of that engine does not miss (the launcher provides it). */
interface Limit { engines: string[]; text: string; browsersOnly: boolean }
interface AppView {
  code: string; name: string; types: TypeGroup[]; associated: boolean; shortcutItems: ShortcutItem[]; browser: string | null; profile: string | null;
  /**
   * Engines the app recommends (and `kynoko`: the Kynoko window itself), and
   * what the user misses in others (its manifest).
   */
  recommended: string[]; limitations: Limit[];
}
interface Account { signedIn: boolean; pending: boolean; email: string | null; name: string | null; notKept: boolean }
interface State {
  apps: AppView[];
  browsers: Browser[];
  /** The browser "the system's" stands for, when known. */
  systemBrowser: string | null;
  defaultBrowser: string | null;
  defaultProfile: string | null;
  version: string;
  catalogueDate: string;
  catalogueChecked: number | null;
  catalogueError: string | null;
  windows: boolean;
  /** 'windows', 'macos' or 'linux': where shortcuts go is named after it. */
  os: string;
  /** The app a `kynoko-launcher://settings?app=` link asked for. */
  focus: string | null;
  /** The section (its id) a `section=` link asked for: it wins over the app. */
  focusSection: string | null;
  /** The first start's question about the files is still to be answered. */
  askAssociations: boolean;
  /** AI assistants the launcher knows, and whether Kynoko is in their configuration. */
  assistants: AssistantView[];
  /** What to add by hand to another assistant's configuration. */
  assistantSnippet: string;
  /** The window is on screen (it lives hidden while the launcher serves files or assistants). */
  shown: boolean;
  /** Assistants connected right now. */
  assistantsOnline: number;
  /** The Kynoko account (docs/SPEC.md, section 20). */
  account: Account;
}
interface AssistantView { id: string; name: string; installed: boolean; connected: boolean }

const lang: Lang = pickLang(navigator.languages);
document.documentElement.lang = lang;
document.documentElement.dir = lang === 'ar' ? 'rtl' : 'ltr';

/* ------------------------------------------------------------------ shell
   Built once: the title bar (the launcher's name, the account, the window's
   buttons), the tabs under it, the update's banner, and the scrolling page.
   A repaint redraws what is inside them, so that the page keeps its scroll
   and the bar its hover. */

const brandVersion = el('span', { class: 'tb-version' });
const accountSlot = el('div', { class: 'tb-account' });
const bar = titleBar(
  { minimize: t(lang, 'WINDOW_MINIMIZE'), maximize: t(lang, 'WINDOW_MAXIMIZE'), restore: t(lang, 'WINDOW_RESTORE'), close: t(lang, 'WINDOW_CLOSE') },
  // Latin words and a number: left to right in every language.
  el('div', { class: 'tb-brand', dir: 'ltr' }, windowIconImg('tb-icon', 18), el('span', { class: 'tb-title' }, 'Kynoko Launcher'), brandVersion),
  el('div', { class: 'tb-fill' }),
  accountSlot,
);
const tabList = el('div', { class: 'tabs', role: 'tablist', ariaLabel: 'Kynoko Launcher' });
const banners = el('div', { class: 'banners' });
const page = el('div', { class: 'page' });
const content = el('main', { class: 'content', id: 'panel', role: 'tabpanel', tabIndex: -1 }, page);
document.getElementById('root')!.replaceChildren(el('div', { class: 'chrome' }, bar, el('div', { class: 'tabs-row' }, tabList)), banners, content);

/* ---------------------------------------------------------------- browsers */

/** The launcher's own window is not a browser: it is named, in the user's language. */
const EMBEDDED = 'kynoko-window';
function labelOf(b: Browser): string {
  return b.id === EMBEDDED ? t(lang, 'EMBEDDED') : b.name;
}

/**
 * The web engine `b` renders pages with, as apps speak of engines (the
 * launcher says: the Kynoko window is WebView2 on Windows, WebKit elsewhere;
 * Opera is Chromium).
 */
function engineOf(b: Browser | undefined): string | null {
  return b && ['chromium', 'gecko', 'webkit'].includes(b.webEngine) ? b.webEngine : null;
}

/**
 * Whether `app` recommends `b`: by the engine it renders with, or, for the
 * Kynoko window, by name (`kynoko`), whatever its engine.
 */
function recommends(app: AppView | null, b: Browser): boolean {
  if (!app) return false;
  if (b.id === EMBEDDED && app.recommended.includes('kynoko')) return true;
  return app.recommended.includes(engineOf(b) ?? '');
}

/**
 * What the user misses in `b`, for `app`: its engine's limitations, unless the
 * app recommends that engine. The Kynoko window is spared those that only
 * browsers have, but keeps its engine's others (WebKit's, on macOS and Linux),
 * even when the app recommends it.
 */
function limitsOf(app: AppView, b: Browser | undefined): Limit[] {
  const engine = engineOf(b);
  if (!b || !engine || app.recommended.includes(engine)) return [];
  return app.limitations.filter((l) => l.engines.includes(engine) && !(b.id === EMBEDDED && l.browsersOnly));
}

/** Well-known browsers of an engine, to suggest when none is installed. */
function suggestionsFor(engine: string, os: string): string[] {
  if (engine === 'chromium') return ['Google Chrome', 'Microsoft Edge', 'Brave'];
  if (engine === 'gecko') return ['Firefox'];
  return engine === 'webkit' && os === 'macos' ? ['Safari'] : [];
}

/** The browser an app opens in: its own, else the default, else the system's. */
function effectiveBrowser(state: State, app: AppView | null): Browser | undefined {
  const id = app?.browser ?? state.defaultBrowser ?? state.systemBrowser;
  return state.browsers.find((b) => b.id === id);
}

/**
 * What a chosen browser needs said, under the place it is chosen: one that
 * cannot give an app window (Opera ignores --app, verified; Safari; Firefox
 * outside Windows), with the way to get one; the Kynoko window's own sign-in,
 * unless the launcher's account signs it in already (section 20).
 */
function browserNotes(state: State, b: Browser | undefined): HTMLElement[] {
  if (!b) return [];
  if (b.id === EMBEDDED) return [el('p', { class: 'note' }, t(lang, state.account.signedIn ? 'EMBEDDED_HINT_SIGNED_IN' : 'EMBEDDED_HINT'))];
  return hasAppMode(b, state.os) ? [] : [el('p', { class: 'note' }, t(lang, 'NO_APP_MODE', { browser: b.name }))];
}

/** Whether the launcher can open `b` as an app window on this system. */
function hasAppMode(b: Browser, os: string): boolean {
  return b.engine === 'chromium' || b.engine === 'embedded' || (b.engine === 'gecko' && os === 'windows');
}

/** A browser choice; for an app that recommends some, those come first, in their own group. */
function browserSelect(state: State, value: string | null, first: string, label: string, onChange: (id: string | null) => void, app: AppView | null = null) {
  const select = el('select', { ariaLabel: label });
  select.append(el('option', { value: '' }, first));
  const option = (b: Browser) => el('option', { value: b.id, selected: b.id === value }, labelOf(b));
  const good = state.browsers.filter((b) => recommends(app, b));
  if (good.length) {
    const others = state.browsers.filter((b) => !good.includes(b));
    select.append(el('optgroup', { label: t(lang, 'RECOMMENDED_GROUP') }, ...good.map(option)));
    if (others.length) select.append(el('optgroup', { label: t(lang, 'OTHER_BROWSERS') }, ...others.map(option)));
  } else {
    for (const b of state.browsers) select.append(option(b));
  }
  select.addEventListener('change', () => onChange(select.value || null));
  return select;
}

/** The profile choice for `browserId`, when that browser has more than one. */
function profileSelect(state: State, browserId: string | null, value: string | null, label: string, onChange: (id: string | null) => void) {
  const profiles = state.browsers.find((b) => b.id === browserId)?.profiles ?? [];
  if (profiles.length < 2) return null;
  const select = el('select', { ariaLabel: label });
  select.append(el('option', { value: '' }, t(lang, 'USUAL_PROFILE')));
  for (const p of profiles) select.append(el('option', { value: p.id, selected: p.id === value }, p.name));
  select.addEventListener('change', () => onChange(select.value || null));
  return select;
}

/* ------------------------------------------------------------- the controls */

/** "3 of 12", or "None": what a closed or open section holds. */
function countOf(n: number, total: number): string {
  return n === 0 ? t(lang, 'NONE') : t(lang, 'COUNT_OF', { n: String(n), total: String(total) });
}

/** "Select all" / "Deselect all", each greyed out when it has nothing to do. */
function bulkOf(key: string, items: { on: boolean }[], set: (on: boolean) => void): HTMLElement | null {
  if (items.length < 2) return null;
  const bulk = (label: string, on: boolean) => {
    const b = keyed(el('button', { type: 'button', class: 'link', disabled: items.every((x) => x.on === on) }, label), `${key}:${on}`);
    b.addEventListener('click', () => set(on));
    return b;
  };
  return el('div', { class: 'bulk' }, bulk(t(lang, 'ALL_TYPES'), true), bulk(t(lang, 'NO_TYPES'), false));
}

/** A Boréal switch: a checkbox with the switch role, its label beside it. */
function switchOf(key: string, checked: boolean, label: string, onChange: (on: boolean) => void, cls = 'k-toggle'): HTMLLabelElement {
  const input = keyed(el('input', { type: 'checkbox', checked, className: 'k-switch' }), key);
  input.setAttribute('role', 'switch');
  input.addEventListener('change', () => onChange(input.checked));
  return el('label', { class: cls }, input, el('span', {}, label));
}

/** A Boréal checkbox with its label. */
function tickOf(key: string, checked: boolean, label: string, cls: string, onChange: (on: boolean) => void): HTMLLabelElement {
  const input = keyed(el('input', { type: 'checkbox', checked, className: 'k-check' }), key);
  input.addEventListener('change', () => onChange(input.checked));
  return el('label', { class: cls }, input, el('span', {}, label));
}

/**
 * The app's file types. The switch associates them; the types themselves
 * are only shown while it is on, by the facade each one opens in.
 */
function filesOf(app: AppView): HTMLElement {
  const all = app.types.flatMap((g) => g.exts);
  const head = el('div', { class: 'section-head' },
    switchOf(`assoc:${app.code}`, app.associated, t(lang, 'ASSOCIATE'), (on) => void run(() => invoke('set_associated', { code: app.code, on }))),
    el('span', { class: 'count' }, countOf(app.associated ? all.filter((x) => x.on).length : 0, all.length)));
  const section = el('section', { class: 'section' }, head);
  if (!app.associated) return section;
  const body = el('div', { class: 'section-body', role: 'group', ariaLabel: t(lang, 'TYPES_FOR', { app: app.name }) });
  for (const group of app.types) {
    // Extensions are technical: left to right whatever the language.
    const exts = el('span', { class: 'chips', dir: 'ltr' });
    for (const x of group.exts) {
      exts.append(tickOf(`ext:${app.code}:${x.ext}`, x.on, '.' + x.ext, 'chip', (on) => void run(() => invoke('set_extension', { code: app.code, ext: x.ext, on }))));
    }
    body.append(el('div', { class: 'type-group' }, el('span', { class: 'facade' }, group.facade), exts));
  }
  const bulk = bulkOf(`exts:${app.code}`, all, (on) => void run(() => invoke('set_extension', { code: app.code, ext: null, on })));
  if (bulk) body.append(bulk);
  section.append(body);
  return section;
}

/**
 * The apps kept ticked in the first start's question, and the ones it has
 * shown, kept across repaints: an app the online catalogue brings while the
 * question is up (a first start begins with the bundled copy) comes ticked.
 */
const welcomeKept = new Set<string>();
const welcomeSeen = new Set<string>();

/**
 * The first start's question (docs/SPEC.md, section 7): should the apps
 * open the user's files? Every app with file types ticked, its types under
 * its name, so that what it replaces is in view; answered once.
 */
function welcomeOf(state: State): HTMLElement | null {
  const apps = state.apps.filter((a) => a.types.length);
  if (!state.askAssociations || !apps.length) return null;
  const kept = welcomeKept;
  for (const app of apps) {
    if (!welcomeSeen.has(app.code)) {
      welcomeSeen.add(app.code);
      kept.add(app.code);
    }
  }
  const list = el('div', { class: 'welcome-apps', role: 'group', ariaLabel: t(lang, 'WELCOME_TITLE') });
  const go = keyed(el('button', { type: 'button', class: 'primary' }, t(lang, 'WELCOME_ASSOCIATE')), 'welcome:go');
  const sync = () => { go.disabled = kept.size === 0; };
  for (const app of apps) {
    const exts = app.types.flatMap((g) => g.exts).map((x) => '.' + x.ext).join(' ');
    const tick = tickOf(`welcome:${app.code}`, kept.has(app.code), app.name, 'welcome-app', (on) => {
      if (on) kept.add(app.code); else kept.delete(app.code);
      sync();
    });
    // Extensions are technical: left to right whatever the language.
    tick.append(el('small', { dir: 'ltr' }, exts));
    list.append(tick);
  }
  sync();
  go.addEventListener('click', () => void run(() => invoke('answer_associations', { codes: [...kept] })));
  const later = keyed(el('button', { type: 'button' }, t(lang, 'WELCOME_LATER')), 'welcome:later');
  later.addEventListener('click', () => void run(() => invoke('answer_associations', { codes: [] })));
  return el('section', { class: 'card welcome' },
    el('h2', {}, t(lang, 'WELCOME_TITLE')),
    el('p', { class: 'note' }, t(lang, state.windows ? 'WELCOME_WINDOWS' : 'WELCOME_DEFAULT')),
    list,
    el('div', { class: 'welcome-actions' }, go, later));
}

/** Which accordions are open, per app (this window only: a convenience). */
function isOpen(id: string): boolean {
  try { return localStorage.getItem('open:' + id) === '1'; } catch { return false; }
}
function setOpen(id: string, open: boolean): void {
  try { localStorage.setItem('open:' + id, open ? '1' : '0'); } catch { /* private storage: stays closed */ }
}

/**
 * The app's shortcuts, as an accordion: closed, it says how many are made;
 * open, one line per shortcut, named as the menu shows it. Ticking one is
 * what adds the app to the menu.
 */
function shortcutsOf(app: AppView, os: string): HTMLElement {
  const title = t(lang, os === 'macos' ? 'SHORTCUT_ITEMS_MACOS' : os === 'linux' ? 'SHORTCUT_ITEMS_LINUX' : 'SHORTCUT_ITEMS_WINDOWS');
  const id = 'shortcuts-' + app.code;
  const open = isOpen(id);
  const bodyId = 'body-' + id;
  const toggle = keyed(el('button', { type: 'button', class: 'accordion' },
    el('span', { class: 'accordion-title' }, title),
    el('span', { class: 'count' }, countOf(app.shortcutItems.filter((i) => i.on).length, app.shortcutItems.length)),
    el('span', { class: open ? 'chevron open' : 'chevron', ariaHidden: 'true' })), `shortcuts:${app.code}`);
  toggle.setAttribute('aria-expanded', String(open));
  toggle.setAttribute('aria-controls', bodyId);
  toggle.addEventListener('click', () => {
    setOpen(id, !open);
    void render();
  });
  const section = el('section', { class: 'section' }, el('div', { class: 'section-head' }, toggle));
  if (!open) return section;
  const body = el('div', { class: 'section-body shortcut-list', id: bodyId, role: 'group', ariaLabel: title });
  // Linux: one menu entry per app, its tools on a right click (see shortcuts.rs).
  if (os === 'linux' && app.shortcutItems.length > 1) body.append(el('p', { class: 'note' }, t(lang, 'SHORTCUTS_LINUX_HINT')));
  for (const item of app.shortcutItems) {
    body.append(tickOf(`shortcut:${app.code}:${item.key}`, item.on, item.name, item.key ? 'shortcut' : 'shortcut main',
      (on) => void run(() => invoke('set_shortcut_item', { code: app.code, key: item.key, on, lang }))));
  }
  const bulk = bulkOf(`shortcuts:${app.code}`, app.shortcutItems, (on) => void run(() => invoke('set_shortcut_item', { code: app.code, key: null, on, lang })));
  if (bulk) body.append(bulk);
  section.append(body);
  return section;
}

/** Icons already asked for, by app code: a re-render reuses them. */
const icons = new Map<string, Promise<string | null>>();

/** The app's icon, filled in when it arrives (never blocks the window). */
function iconOf(code: string): HTMLElement {
  const img = el('img', { class: 'app-icon', alt: '', width: 32, height: 32 });
  img.hidden = true;
  if (!icons.has(code)) icons.set(code, invoke<string | null>('app_icon', { code }).catch(() => null));
  void icons.get(code)!.then((src) => {
    if (src) {
      img.src = src;
      img.hidden = false;
    }
  });
  return img;
}

/* ----------------------------------------------------- check and update */

/** What a check answers (see check_catalogue). */
interface CheckReport { catalogue: string; catalogueError: string | null; current: string; latest: string | null; newer: boolean; installable: boolean }
let checking = false;
let lastCheck: CheckReport | null = null;
/** When the last check started: the window checks by itself when it shows,
 *  at most once an hour (AUTO_CHECK_EVERY); "Check now" any time. */
let checkedAt = 0;
const AUTO_CHECK_EVERY = 3_600_000;

/** Asks for the catalogue and the latest release (check_catalogue). */
function check(version: string): void {
  if (checking || installing()) return;
  checking = true;
  checkedAt = Date.now();
  void render();
  void invoke<CheckReport>('check_catalogue')
    .then((r) => { lastCheck = r; }, (e) => { lastCheck = { catalogue: 'failed', catalogueError: String(e), current: version, latest: null, newer: false, installable: false }; })
    .finally(() => { checking = false; void render(); });
}

/** The check the window makes by itself when it shows (never while hidden). */
async function autoCheck(): Promise<void> {
  if (Date.now() - checkedAt < AUTO_CHECK_EVERY) return;
  const state = await invoke<State>('get_state', { lang });
  if (state.shown) check(state.version);
}

/** The newer version downloaded and checked (update_download). */
interface UpdateReady { version: string; mode: 'restart' | 'handoff'; windows: number; files: number }
/** Where "Download and install" is. */
type Install =
  | { phase: 'downloading'; received: number; total: number }
  | { phase: 'waiting' }
  | { phase: 'flushing'; seconds: number }
  | { phase: 'restarting' }
  | { phase: 'handed' }
  | { phase: 'cancelled' }
  | { phase: 'failed'; error: string };
let install: Install | null = null;
/** The system the window runs on (State.os), as of the last render. */
let currentOs = '';
/** An install under way: the buttons wait. */
function installing(): boolean {
  return !!install && !['handed', 'cancelled', 'failed'].includes(install.phase);
}

function installText(i: Install): string {
  switch (i.phase) {
    case 'downloading': {
      const ratio = i.total ? i.received / i.total : 0;
      return t(lang, 'INSTALL_DOWNLOADING', { percent: new Intl.NumberFormat(lang, { style: 'percent' }).format(ratio) });
    }
    case 'waiting': return t(lang, 'INSTALL_WAITING');
    case 'flushing': return t(lang, 'INSTALL_FLUSHING', { seconds: String(i.seconds) });
    case 'restarting': return t(lang, 'INSTALL_RESTARTING');
    case 'handed': return t(lang, currentOs === 'macos' ? 'INSTALL_HANDED_MACOS' : 'INSTALL_HANDED_LINUX');
    case 'cancelled': return t(lang, 'INSTALL_CANCELLED');
    case 'failed': return t(lang, 'INSTALL_FAILED', { error: i.error });
  }
}

/** The install's line under the announcement: its words, the download's bar,
 *  and while the Kynoko windows are awaited, a way to give up. */
function installLine(i: Install): HTMLElement {
  const line = el('div', { class: `install ${i.phase}`, id: 'install-line', role: 'status' }, el('span', { class: 'install-text' }, installText(i)));
  // The windows are closed and the web engine is given time to write: the
  // user may choose not to wait.
  if (i.phase === 'flushing') {
    const now = el('button', { type: 'button', class: 'primary' }, t(lang, 'INSTALL_NOW'));
    now.addEventListener('click', () => { now.disabled = true; void invoke('update_now'); });
    line.append(now);
  }
  if (i.phase === 'waiting' || i.phase === 'flushing') {
    const cancel = el('button', { type: 'button' }, t(lang, 'INSTALL_CANCEL'));
    cancel.addEventListener('click', () => void invoke('update_cancel'));
    line.append(cancel);
  }
  if (i.phase === 'downloading') line.append(el('progress', { max: i.total || 1, value: i.received }));
  return line;
}

/** Repaints the install's line alone: the download reports several times a second. */
function paintInstall(): void {
  const line = document.getElementById('install-line');
  if (line && install) line.replaceWith(installLine(install));
  else void render();
}

/**
 * "Download and install": the release's file for this system, downloaded
 * and checked by the launcher (update.rs), then installed. Installing in
 * place closes the launcher and its Kynoko windows (each asks about unsaved
 * work first) and starts the new version: said first when something is open.
 */
async function startInstall(): Promise<void> {
  install = { phase: 'downloading', received: 0, total: 0 };
  await render();
  let ready: UpdateReady;
  try {
    ready = await invoke<UpdateReady>('update_download');
  } catch (e) {
    install = { phase: 'failed', error: String(e) };
    await render();
    return;
  }
  if (ready.mode === 'restart' && (ready.windows > 0 || ready.files > 0)) {
    const said = [t(lang, 'INSTALL_CLOSES', { version: ready.version })];
    if (ready.windows > 0) said.push(t(lang, 'INSTALL_WINDOWS'));
    if (ready.files > 0) said.push(t(lang, 'INSTALL_FILES'));
    if (!(await confirmDialog(said, t(lang, 'INSTALL_NOW'), t(lang, 'INSTALL_CANCEL')))) {
      install = { phase: 'cancelled' };
      await render();
      return;
    }
  }
  install = { phase: ready.mode === 'restart' && ready.windows > 0 ? 'waiting' : 'restarting' };
  paintInstall();
  try {
    const outcome = await invoke<string>('update_install');
    install = outcome === 'handed' ? { phase: 'handed' } : outcome === 'cancelled' ? { phase: 'cancelled' } : { phase: 'restarting' };
  } catch (e) {
    install = { phase: 'failed', error: String(e) };
  }
  await render();
}

/**
 * The newer version, in a banner under the tabs, whatever the tab: its name
 * and its two buttons on one line, "Download and install" where this system
 * has a file in the release, "Download" (the release's page on GitHub)
 * always; under them, where an install is.
 */
function updateBanner(c: CheckReport): HTMLElement {
  const busy = installing();
  const actions = el('div', { class: 'update-actions' });
  if (c.installable) {
    const both = keyed(el('button', { type: 'button', class: 'primary', disabled: busy }, t(lang, 'INSTALL_NEW')), 'update:install');
    both.addEventListener('click', () => void startInstall());
    actions.append(both);
  }
  const download = keyed(el('button', { type: 'button', class: c.installable ? 'from' : 'primary from', disabled: busy },
    t(lang, 'DOWNLOAD_NEW'), el('small', {}, t(lang, 'DOWNLOAD_FROM'))), 'update:download');
  download.addEventListener('click', () => void invoke('open_download'));
  actions.append(download);
  const inner = el('div', { class: 'banner-inner' },
    el('div', { class: 'update-row' },
      el('p', { class: 'update-title' }, icon(ICONS.sparkle, 18), el('span', {}, t(lang, 'APP_NEWER', { version: c.latest ?? '' }))),
      actions));
  if (install) inner.append(installLine(install));
  return el('section', { class: 'banner update', role: 'status' }, inner);
}

/**
 * The catalogue's trouble, said quietly with the last success: never a
 * notification. Null when there is none.
 */
function catalogueTrouble(state: State): string | null {
  const c = lastCheck;
  const last = state.catalogueChecked ? new Date(state.catalogueChecked * 1000).toLocaleString(lang) : null;
  return c?.catalogue === 'failed' ? t(lang, 'CAT_FAILED', { error: c.catalogueError ?? '' })
    : state.catalogueError ? (last ? t(lang, 'CATALOGUE_FAILED', { date: last }) : t(lang, 'CATALOGUE_BUNDLED'))
    : !state.catalogueChecked && !checking ? t(lang, 'CATALOGUE_BUNDLED')
    : null;
}

/* ------------------------------------------------------------- the account */

/** The account's panel under its button, open or not (kept across repaints). */
let accountOpen = false;
/** What the account's corner was last drawn from: redrawn only when it changes. */
let accountDrawn = '';
let lastState: State | null = null;

/** One or two letters for the account's badge: the name's initials, else the address's first. */
function initialsOf(a: Account): string {
  const words = (a.name ?? '').trim().split(/\s+/).filter(Boolean);
  const first = (w: string) => Array.from(w)[0] ?? '';
  if (words.length) return (first(words[0]) + (words.length > 1 ? first(words[words.length - 1]) : '')).toLocaleUpperCase(lang);
  return first(a.email ?? '').toLocaleUpperCase(lang);
}

function avatarOf(a: Account, cls: string): HTMLElement {
  const letters = initialsOf(a);
  return el('span', { class: `avatar ${cls}`, ariaHidden: 'true' }, letters || icon(ICONS.person, 14));
}

function setAccountOpen(open: boolean, focusButton = false): void {
  accountOpen = open;
  if (lastState) paintAccount(lastState, true);
  if (focusButton) accountSlot.querySelector<HTMLElement>('.account-btn')?.focus();
}

/**
 * The Kynoko account (docs/SPEC.md, section 20), in the title bar. Signed
 * out: "Sign in", which signs in at once (what it gives in its tooltip).
 * Signing in: the button waits, and its panel says where to finish, with
 * Cancel. Signed in: the account's badge and name; its panel says who, what
 * the sign-in does, and "Sign out".
 */
function paintAccount(state: State, force = false): void {
  const a = state.account;
  const drawn = JSON.stringify([a, accountOpen]);
  if (!force && drawn === accountDrawn) return;
  accountDrawn = drawn;
  if (!a.signedIn && !a.pending) {
    const signIn = keyed(el('button', { type: 'button', class: 'account-btn sign-in' }, icon(ICONS.person, 15), el('span', {}, t(lang, 'ACCOUNT_SIGN_IN'))), 'account');
    signIn.addEventListener('click', () => {
      // The panel says where to finish when the user comes back.
      accountOpen = true;
      void run(() => invoke('account_sign_in'));
    });
    accountSlot.replaceChildren(signIn);
    tooltip(signIn, t(lang, 'ACCOUNT_SIGNED_OUT'), accountSlot);
    return;
  }
  const who = a.name ?? a.email;
  const button = keyed(el('button', { type: 'button', class: a.notKept ? 'account-btn warn' : 'account-btn' },
    ...(a.pending
      ? [el('span', { class: 'spinner', ariaHidden: 'true' }), el('span', { class: 'who' }, t(lang, 'ACCOUNT_SIGN_IN'))]
      : [avatarOf(a, 'small'), ...(who ? [el('span', { class: 'who', dir: 'auto' }, who)] : [])]),
    el('span', { class: accountOpen ? 'caret open' : 'caret' }, icon(ICONS.chevronDown, 14))), 'account');
  button.ariaLabel = a.pending ? `${t(lang, 'ACCOUNT_MENU')}: ${t(lang, 'ACCOUNT_PENDING')}`
    : `${t(lang, 'ACCOUNT_MENU')}: ${a.email ? t(lang, 'ACCOUNT_SIGNED_IN', { account: a.email }) : t(lang, 'ACCOUNT_SIGNED_IN_ANON')}`;
  button.setAttribute('aria-haspopup', 'dialog');
  button.setAttribute('aria-expanded', String(accountOpen));
  button.addEventListener('click', () => setAccountOpen(!accountOpen));
  const parts: HTMLElement[] = [button];
  if (accountOpen) parts.push(accountPanel(a));
  accountSlot.replaceChildren(...parts);
}

function accountPanel(a: Account): HTMLElement {
  const panel = el('div', { class: 'account-panel', role: 'dialog', ariaLabel: t(lang, 'ACCOUNT_MENU') });
  if (a.pending) {
    const cancel = keyed(el('button', { type: 'button' }, t(lang, 'ACCOUNT_CANCEL')), 'account:cancel');
    cancel.addEventListener('click', () => {
      accountOpen = false;
      void run(() => invoke('account_cancel'));
    });
    panel.append(
      el('div', { class: 'panel-row' }, el('span', { class: 'spinner', ariaHidden: 'true' }), el('p', {}, t(lang, 'ACCOUNT_PENDING'))),
      el('div', { class: 'panel-actions' }, cancel));
    return panel;
  }
  const out = keyed(el('button', { type: 'button', class: 'with-icon' }, icon(ICONS.signOut, 16), el('span', {}, t(lang, 'ACCOUNT_SIGN_OUT'))), 'account:out');
  out.addEventListener('click', () => {
    out.disabled = true;
    accountOpen = false;
    void run(() => invoke('account_sign_out'));
  });
  const name = a.name ?? a.email;
  panel.append(
    el('div', { class: 'panel-who' },
      avatarOf(a, 'large'),
      el('div', { class: 'who-text' },
        el('strong', { dir: 'auto' }, name ?? t(lang, 'ACCOUNT_SIGNED_IN_ANON')),
        ...(a.name && a.email ? [el('span', { class: 'note', dir: 'auto' }, a.email)] : []))),
    el('p', { class: 'note panel-line' }, icon(ICONS.check, 15), el('span', {}, t(lang, 'ACCOUNT_CARRIES'))),
    ...(a.notKept ? [el('p', { class: 'note panel-warn', role: 'status' }, t(lang, 'ACCOUNT_NOT_KEPT'))] : []),
    el('div', { class: 'panel-actions' }, out));
  return panel;
}

// The panel closes with Escape and with a press anywhere else.
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && accountOpen && !document.querySelector('dialog[open]')) setAccountOpen(false, true);
});
document.addEventListener('mousedown', (e) => {
  if (accountOpen && e.target instanceof Node && !accountSlot.contains(e.target)) setAccountOpen(false);
});

/* ---------------------------------------------------------------- the tabs */

type Tab = 'apps' | 'assistants' | 'general';
const TABS: Tab[] = ['apps', 'assistants', 'general'];

function storedTab(): Tab {
  try {
    const kept = localStorage.getItem('tab');
    return TABS.includes(kept as Tab) ? (kept as Tab) : 'apps';
  } catch { return 'apps'; }
}
/** The tab shown (this window only: a convenience); a link's app or section picks its own. */
let tab: Tab = storedTab();

function showTab(next: Tab, focus = false): void {
  if (next !== tab) {
    tab = next;
    try { localStorage.setItem('tab', next); } catch { /* private storage: back to the apps next time */ }
    content.scrollTop = 0;
  }
  void render().then(() => { if (focus) document.getElementById('tab-' + next)?.focus(); });
}

function paintTabs(state: State): void {
  const label: Record<Tab, string> = { apps: t(lang, 'APPS_TITLE'), assistants: t(lang, 'ASSISTANTS_TITLE'), general: t(lang, 'TAB_GENERAL') };
  const glyph: Record<Tab, readonly string[]> = { apps: ICONS.apps, assistants: ICONS.sparkle, general: ICONS.gear };
  tabList.replaceChildren(...TABS.map((id) => {
    const selected = id === tab;
    const b = keyed(el('button', { type: 'button', class: 'tab', id: 'tab-' + id, role: 'tab', tabIndex: selected ? 0 : -1 },
      icon(glyph[id], 16), el('span', {}, label[id])), 'tab:' + id);
    // Assistants at work right now: their number on the tab.
    if (id === 'assistants' && state.assistantsOnline > 0) b.append(el('span', { class: 'badge' }, String(state.assistantsOnline)));
    b.setAttribute('aria-selected', String(selected));
    b.setAttribute('aria-controls', 'panel');
    b.addEventListener('click', () => showTab(id));
    // The arrows move between the tabs, in the reading direction.
    b.addEventListener('keydown', (e) => {
      const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key] ?? 0;
      const to = e.key === 'Home' ? 0 : e.key === 'End' ? TABS.length - 1
        : step ? (TABS.indexOf(id) + step * (lang === 'ar' ? -1 : 1) + TABS.length) % TABS.length : -1;
      if (to < 0) return;
      e.preventDefault();
      showTab(TABS[to], true);
    });
    return b;
  }));
  content.setAttribute('aria-labelledby', 'tab-' + tab);
}

/* ------------------------------------------------------------ the apps tab */

function appsTab(state: State): HTMLElement[] {
  const parts: HTMLElement[] = [el('p', { class: 'lead' }, t(lang, 'LEAD'))];
  const welcome = welcomeOf(state);
  if (welcome) parts.push(welcome);
  const trouble = catalogueTrouble(state);
  if (trouble) parts.push(el('p', { class: 'note catalogue-note' }, trouble));

  // The default browser: one line, what every app follows unless it has its own.
  const defaultBrowser = el('div', { class: 'browser-default' },
    el('div', { class: 'field' },
      el('span', { class: 'field-label' }, t(lang, 'DEFAULT_BROWSER')),
      keyed(browserSelect(state, state.defaultBrowser, t(lang, 'SYSTEM_DEFAULT'), t(lang, 'DEFAULT_BROWSER'),
        (id) => void run(() => invoke('set_default_browser', { id }))), 'default-browser'),
      ...[profileSelect(state, state.defaultBrowser, state.defaultProfile, t(lang, 'USUAL_PROFILE'),
        (id) => void run(() => invoke('set_default_profile', { id })))].filter((x): x is HTMLSelectElement => !!x).map((s) => keyed(s, 'default-profile'))));
  if (state.browsers.length < 2) defaultBrowser.append(el('p', { class: 'note' }, t(lang, 'NO_BROWSERS')));
  // The default browser's notes; an app's own browser has them under the app.
  const followed = effectiveBrowser(state, null);
  defaultBrowser.append(...browserNotes(state, state.browsers.find((b) => b.id === state.defaultBrowser)));
  parts.push(defaultBrowser);

  const apps = el('div', { class: 'apps' });
  for (const app of state.apps) apps.append(appCard(state, app, followed));
  parts.push(apps);
  if (state.apps.some((a) => a.associated)) {
    parts.push(el('p', { class: 'note' }, t(lang, 'TYPES_HINT')));
    // Windows lets no program make itself the default: where the user does it.
    if (state.windows) {
      const settings = keyed(el('button', { type: 'button' }, t(lang, 'DEFAULT_APPS_BUTTON')), 'default-apps');
      settings.addEventListener('click', () => void run(() => invoke('open_default_apps')));
      parts.push(el('div', { class: 'callout' }, icon(ICONS.info, 18), el('p', {}, t(lang, 'DEFAULT_APPS_NOTE')), settings));
    }
  }
  return parts;
}

function appCard(state: State, app: AppView, followed: Browser | undefined): HTMLElement {
  // The app itself, icon and name, is what opens it.
  const open = keyed(el('button', { type: 'button', class: 'name', ariaLabel: t(lang, 'OPEN_APP', { app: app.name }) },
    iconOf(app.code),
    el('span', { class: 'label' }, el('span', {}, app.name), el('span', { class: 'open' }, t(lang, 'OPEN')))), `open:${app.code}`);
  open.addEventListener('click', () => void run(() => invoke('launch', { target: app.code })));

  // The rest of the header opens the app's settings: closed, it says what
  // is set; the app a menu entry asked for comes open.
  const cardId = 'app-' + app.code;
  const expanded = app.code === state.focus || isOpen(cardId);
  const typesOn = app.associated ? app.types.flatMap((g) => g.exts).filter((x) => x.on).length : 0;
  const typesAll = app.types.flatMap((g) => g.exts).length;
  const summary = [
    ...(typesAll ? [t(lang, 'SUMMARY_FILES', { count: countOf(typesOn, typesAll) })] : []),
    t(lang, 'SUMMARY_SHORTCUTS', { count: countOf(app.shortcutItems.filter((i) => i.on).length, app.shortcutItems.length) }),
  ];
  const bodyId = 'settings-' + app.code;
  const expand = keyed(el('button', { type: 'button', class: 'expand', ariaLabel: t(lang, 'SETTINGS_FOR', { app: app.name }) },
    // One line per setting, stacked.
    el('span', { class: 'summary' }, ...summary.map((line) => el('span', {}, line))),
    el('span', { class: expanded ? 'chevron open' : 'chevron', ariaHidden: 'true' })), `expand:${app.code}`);
  expand.setAttribute('aria-expanded', String(expanded));
  expand.setAttribute('aria-controls', bodyId);
  expand.addEventListener('click', () => {
    setOpen(cardId, !expanded);
    void render();
  });

  const card = el('article', { class: app.code === state.focus ? 'app focus' : 'app', id: cardId },
    el('div', { class: 'app-head' },
      open,
      // The app's browser, on its line: the choice made most often.
      el('span', { class: 'controls' },
        keyed(browserSelect(state, app.browser, t(lang, 'FOLLOW_DEFAULT'), t(lang, 'BROWSER_FOR', { app: app.name }),
          (id) => void run(() => invoke('set_app_browser', { code: app.code, id })), app), `browser:${app.code}`),
        ...[profileSelect(state, app.browser, app.profile, t(lang, 'PROFILE_FOR', { app: app.name }),
          (id) => void run(() => invoke('set_app_profile', { code: app.code, id })))].filter((x): x is HTMLSelectElement => !!x).map((s) => keyed(s, `profile:${app.code}`))),
      expand));
  // What the app says the user will miss in the browser it opens in.
  const browser = effectiveBrowser(state, app);
  // Its own browser's notes, unless the default's, said above already.
  if (app.browser && browser?.id !== followed?.id) card.append(...browserNotes(state, browser));
  const limits = limitsOf(app, browser);
  // None of the browsers the app recommends is installed: some are named,
  // here, where the app's browser is chosen. (The Kynoko window always is.)
  const names = app.recommended.length && !state.browsers.some((b) => recommends(app, b))
    ? app.recommended.flatMap((e) => suggestionsFor(e, state.os))
    : [];
  const suggestion = names.length
    ? t(lang, 'RECOMMEND_INSTALL', { browsers: new Intl.ListFormat(lang, { type: 'disjunction' }).format(names) })
    : null;
  if (browser && limits.length) {
    card.append(el('div', { class: 'limits', role: 'note' },
      el('strong', {}, t(lang, 'LIMITS_WITH', { browser: labelOf(browser) })),
      ...limits.map((l) => el('span', {}, l.text)),
      ...(suggestion ? [el('span', { class: 'suggest-line' }, suggestion)] : [])));
  } else if (suggestion) {
    card.append(el('p', { class: 'suggest', role: 'note' }, suggestion));
  }
  if (expanded) {
    card.append(el('div', { class: 'app-body', id: bodyId },
      // File types: only for an app that opens files.
      ...(app.types.length ? [filesOf(app)] : []),
      ...(app.shortcutItems.length ? [shortcutsOf(app, state.os)] : [])));
  }
  return card;
}

/* ------------------------------------------------------ the assistants tab */

/** What the last change to an assistant's configuration calls for (restart it, add Kynoko by hand). */
let assistantNote: string | null = null;

/**
 * The AI assistants installed on this computer that may use the apps (MCP):
 * one switch each puts Kynoko in its configuration or takes it out; any
 * other assistant gets what to add by hand.
 */
function assistantsTab(state: State): HTMLElement[] {
  const section = el('section', { class: 'assistants', id: 'assistants' });
  section.append(el('p', { class: 'lead' }, t(lang, 'ASSISTANTS_LEAD')));
  const shown = state.assistants.filter((a) => a.installed || a.connected);
  if (shown.length) {
    const list = el('div', { class: 'assistant-list card' });
    for (const a of shown) {
      list.append(switchOf(`assistant:${a.id}`, a.connected, a.name, (on) => void run(async () => {
        try {
          await invoke('set_assistant', { id: a.id, on });
          assistantNote = on ? t(lang, 'ASSISTANTS_RESTART') : null;
        } catch (e) {
          assistantNote = String(e).includes('NOT_PLAIN_JSON')
            ? t(lang, 'ASSISTANTS_NOT_JSON', { assistant: a.name })
            : t(lang, 'ASSISTANTS_FAILED', { error: String(e) });
        }
      }), 'k-toggle assistant'));
    }
    section.append(list);
  } else {
    section.append(el('p', { class: 'callout' }, icon(ICONS.info, 18), el('span', {}, t(lang, 'ASSISTANTS_NONE'))));
  }
  if (assistantNote) section.append(el('p', { class: 'note', role: 'status' }, assistantNote));
  if (state.assistantsOnline > 0) section.append(el('p', { class: 'note online' }, el('span', { class: 'live', ariaHidden: 'true' }), t(lang, 'ASSISTANTS_ONLINE', { n: String(state.assistantsOnline) })));
  // A path and JSON: read left to right in every language.
  const snippet = el('pre', { class: 'snippet', dir: 'ltr' }, state.assistantSnippet);
  const copy = keyed(el('button', { type: 'button' }, t(lang, 'ASSISTANTS_COPY')), 'assistants:copy');
  copy.addEventListener('click', () => {
    void navigator.clipboard.writeText(state.assistantSnippet).then(() => { copy.textContent = t(lang, 'ASSISTANTS_COPIED'); });
  });
  const manual = el('details', { class: 'manual card' },
    el('summary', {}, t(lang, 'ASSISTANTS_MANUAL')),
    el('p', { class: 'note' }, t(lang, 'ASSISTANTS_MANUAL_HINT')),
    snippet,
    copy);
  // Kept open across repaints (a switch turned repaints the window).
  manual.open = manualOpen;
  manual.addEventListener('toggle', () => { manualOpen = manual.open; });
  section.append(manual);
  return [section];
}
let manualOpen = false;

/* --------------------------------------------------------- the general tab */

/**
 * The launcher itself: its version, whether it is up to date, the
 * catalogue's date and "Check now"; then the reset, set apart and said in
 * full: the launcher stays, everything it did to the system goes, and its
 * settings with it.
 */
function generalTab(state: State): HTMLElement[] {
  const c = lastCheck;
  const status: HTMLElement[] = [];
  // While a check runs, its button says so; a newer version has its banner.
  if (!checking && c && !c.newer) {
    status.push(el('span', { class: c.latest ? 'state ok' : 'state' }, ...(c.latest ? [icon(ICONS.check, 14)] : []), t(lang, c.latest ? 'UPDATE_NONE' : 'UPDATE_UNKNOWN')));
  }
  status.push(el('span', {}, t(lang, 'CATALOGUE', { date: new Date(state.catalogueDate).toLocaleDateString(lang) })));
  const button = keyed(el('button', { type: 'button', class: checking ? 'with-icon busy' : 'with-icon', disabled: checking || installing() },
    icon(ICONS.refresh, 16), el('span', {}, t(lang, checking ? 'CHECKING' : 'CHECK_NOW'))), 'check');
  button.addEventListener('click', () => check(state.version));
  const about = el('section', { class: 'card about' },
    el('div', { class: 'about-row' },
      windowIconImg('about-icon', 44),
      el('div', { class: 'about-text' },
        // Latin words and a number: left to right in every language.
        el('h2', { dir: 'ltr' }, 'Kynoko Launcher', el('span', { class: 'version' }, state.version)),
        el('p', { class: 'note status' }, ...status.flatMap((s, i) => (i ? [el('span', { class: 'sep', ariaHidden: 'true' }, '·'), s] : [s])))),
      button));
  const trouble = catalogueTrouble(state);
  if (trouble) about.append(el('p', { class: 'note catalogue-note' }, trouble));

  const remove = keyed(el('button', { type: 'button', class: 'danger' }, t(lang, 'REMOVE_ALL')), 'reset');
  remove.addEventListener('click', () => void (async () => {
    if (!(await confirmDialog([t(lang, 'REMOVE_CONFIRM')], t(lang, 'REMOVE_ALL'), t(lang, 'INSTALL_CANCEL'), { danger: true }))) return;
    await run(async () => {
      await invoke('remove_everything');
      toast(t(lang, 'REMOVED'), 'info', t(lang, 'WINDOW_CLOSE'));
    });
  })());
  return [about, el('section', { class: 'card reset' }, el('p', { class: 'note' }, t(lang, 'REMOVE_NOTE')), remove)];
}

/* -------------------------------------------------------------- the repaint */

/** Does what a control asked, says a failure, and repaints. */
async function run(action: () => Promise<unknown>): Promise<void> {
  try {
    await action();
  } catch (e) {
    toast(String(e), 'error', t(lang, 'WINDOW_CLOSE'));
  }
  await render();
}

async function render(): Promise<void> {
  const state = await invoke<State>('get_state', { lang });
  lastState = state;
  currentOs = state.os;
  // A link named a section or an app (an app's menu entry): its tab. The
  // first start's question is on the apps' tab.
  if (state.focusSection === 'assistants') tab = 'assistants';
  else if (state.focus || (state.askAssociations && !askedOnce)) tab = 'apps';
  if (state.askAssociations) askedOnce = true;
  // The control that had the focus, to give it back once redrawn.
  const active = document.activeElement;
  const focusKey = active instanceof HTMLElement ? active.dataset.key ?? null : null;

  brandVersion.textContent = state.version;
  paintAccount(state);
  paintTabs(state);
  banners.replaceChildren(...(lastCheck?.newer && lastCheck.latest ? [updateBanner(lastCheck)] : []));
  page.replaceChildren(...(tab === 'apps' ? appsTab(state) : tab === 'assistants' ? assistantsTab(state) : generalTab(state)));

  if (focusKey && (document.activeElement === document.body || !document.activeElement)) {
    document.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }
  if (state.focusSection) document.getElementById(state.focusSection)?.scrollIntoView({ block: 'start' });
  else if (state.focus) document.getElementById('app-' + state.focus)?.scrollIntoView({ block: 'center' });
}
/** The first start's question has been shown once (its tab chosen for it then, not after). */
let askedOnce = false;

// The window was asked for (started, or an app's menu entry): show that
// app, and check for a newer version if that was not done in the last hour.
void listen('focus-app', () => { void render().then(() => autoCheck()); });
// The Kynoko account changed (signed in, out, the sign-in window closed).
void listen('account-changed', () => void render());
void listen<string>('account-error', (e) => {
  accountOpen = false;
  toast(e.payload === 'access_denied' ? t(lang, 'ACCOUNT_DENIED') : t(lang, 'ACCOUNT_FAILED', { error: e.payload }), 'error', t(lang, 'WINDOW_CLOSE'));
  void render();
});
// The catalogue changed in the background: show the new one.
void listen('catalogue-updated', () => void render());
// Something written in the background (the first start's shortcuts): show it.
void listen('state-changed', () => void render());
// Where "Download and install" is (update_download, update_install).
void listen<{ received: number; total: number }>('update-progress', (e) => {
  if (install?.phase !== 'downloading') return;
  install = { phase: 'downloading', ...e.payload };
  paintInstall();
});
void listen<number>('update-waiting', () => {
  install = { phase: 'waiting' };
  paintInstall();
});
void listen<number>('update-flushing', (e) => {
  install = { phase: 'flushing', seconds: e.payload };
  paintInstall();
});

// The window may already be on screen when its page starts.
void render().then(() => void autoCheck());
