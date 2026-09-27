import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Lang, pickLang, t } from './i18n';

interface Profile { id: string; name: string; default: boolean }
interface Browser { id: string; name: string; engine: string; profiles: Profile[] }
interface TypeGroup { facade: string; exts: { ext: string; on: boolean }[] }
interface ShortcutItem { key: string; name: string; on: boolean }
interface AppView { code: string; name: string; types: TypeGroup[]; associated: boolean; shortcutItems: ShortcutItem[]; browser: string | null; profile: string | null }
interface State {
  apps: AppView[];
  browsers: Browser[];
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
}

const lang: Lang = pickLang(navigator.languages);
document.documentElement.lang = lang;
document.documentElement.dir = lang === 'ar' ? 'rtl' : 'ltr';

const root = document.getElementById('root')!;

/** Builds an element; text goes through textContent, never innerHTML. */
function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<HTMLElementTagNameMap[K]> & { class?: string } = {},
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  const { class: cls, ...rest } = props;
  if (cls) node.className = cls;
  Object.assign(node, rest);
  node.append(...children);
  return node;
}

/** The launcher's own window is not a browser: it is named, in the user's language. */
const EMBEDDED = 'kynoko-window';
function labelOf(b: Browser): string {
  return b.id === EMBEDDED ? t(lang, 'EMBEDDED') : b.name;
}

/** Whether the launcher can open `b` as an app window on this system. */
function hasAppMode(b: Browser, os: string): boolean {
  return b.engine === 'chromium' || b.engine === 'embedded' || (b.engine === 'gecko' && os === 'windows');
}

function browserSelect(state: State, value: string | null, first: string, label: string, onChange: (id: string | null) => void) {
  const select = el('select', { ariaLabel: label });
  select.append(el('option', { value: '' }, first));
  for (const b of state.browsers) select.append(el('option', { value: b.id, selected: b.id === value }, labelOf(b)));
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

/** "3 of 12", or "None": what a closed or open section holds. */
function countOf(n: number, total: number): string {
  return n === 0 ? t(lang, 'NONE') : t(lang, 'COUNT_OF', { n: String(n), total: String(total) });
}

/** "Select all" / "Deselect all", each greyed out when it has nothing to do. */
function bulkOf(items: { on: boolean }[], set: (on: boolean) => void): HTMLElement | null {
  if (items.length < 2) return null;
  const bulk = (label: string, on: boolean) => {
    const b = el('button', { type: 'button', class: 'link', disabled: items.every((x) => x.on === on) }, label);
    b.addEventListener('click', () => set(on));
    return b;
  };
  return el('div', { class: 'bulk' }, bulk(t(lang, 'ALL_TYPES'), true), bulk(t(lang, 'NO_TYPES'), false));
}

/** A Boréal switch: a checkbox with the switch role, its label beside it. */
function switchOf(checked: boolean, label: string, onChange: (on: boolean) => void): HTMLLabelElement {
  const input = el('input', { type: 'checkbox', checked, className: 'k-switch' });
  input.setAttribute('role', 'switch');
  input.addEventListener('change', () => onChange(input.checked));
  return el('label', { class: 'k-toggle' }, input, el('span', {}, label));
}

/** A Boréal checkbox with its label. */
function tickOf(checked: boolean, label: string, cls: string, onChange: (on: boolean) => void): HTMLLabelElement {
  const input = el('input', { type: 'checkbox', checked, className: 'k-check' });
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
    switchOf(app.associated, t(lang, 'ASSOCIATE'), (on) => void run(() => invoke('set_associated', { code: app.code, on }))),
    el('span', { class: 'count' }, countOf(app.associated ? all.filter((x) => x.on).length : 0, all.length)));
  const section = el('section', { class: 'section' }, head);
  if (!app.associated) return section;
  const body = el('div', { class: 'section-body', role: 'group', ariaLabel: t(lang, 'TYPES_FOR', { app: app.name }) });
  for (const group of app.types) {
    // Extensions are technical: left to right whatever the language.
    const exts = el('span', { class: 'chips', dir: 'ltr' });
    for (const x of group.exts) {
      exts.append(tickOf(x.on, '.' + x.ext, 'chip', (on) => void run(() => invoke('set_extension', { code: app.code, ext: x.ext, on }))));
    }
    body.append(el('div', { class: 'type-group' }, el('span', { class: 'facade' }, group.facade), exts));
  }
  const bulk = bulkOf(all, (on) => void run(() => invoke('set_extension', { code: app.code, ext: null, on })));
  if (bulk) body.append(bulk);
  section.append(body);
  return section;
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
  const toggle = el('button', { type: 'button', class: 'accordion' },
    el('span', { class: 'accordion-title' }, title),
    el('span', { class: 'count' }, countOf(app.shortcutItems.filter((i) => i.on).length, app.shortcutItems.length)),
    el('span', { class: open ? 'chevron open' : 'chevron', ariaHidden: 'true' }));
  toggle.setAttribute('aria-expanded', String(open));
  toggle.setAttribute('aria-controls', bodyId);
  toggle.addEventListener('click', () => {
    setOpen(id, !open);
    void render();
  });
  const section = el('section', { class: 'section' }, el('div', { class: 'section-head' }, toggle));
  if (!open) return section;
  const body = el('div', { class: 'section-body shortcut-list', id: bodyId, role: 'group', ariaLabel: title });
  for (const item of app.shortcutItems) {
    body.append(tickOf(item.on, item.name, item.key ? 'shortcut' : 'shortcut main',
      (on) => void run(() => invoke('set_shortcut_item', { code: app.code, key: item.key, on, lang }))));
  }
  const bulk = bulkOf(app.shortcutItems, (on) => void run(() => invoke('set_shortcut_item', { code: app.code, key: null, on, lang })));
  if (bulk) body.append(bulk);
  section.append(body);
  return section;
}

/** Icons already asked for, by app code: a re-render reuses them. */
const icons = new Map<string, Promise<string | null>>();

/** The app's icon, filled in when it arrives (never blocks the window). */
function iconOf(code: string): HTMLElement {
  const img = el('img', { class: 'app-icon', alt: '', width: 28, height: 28 });
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

async function run(action: () => Promise<unknown>): Promise<void> {
  try {
    await action();
  } catch (e) {
    root.append(el('p', { class: 'error', role: 'alert' }, String(e)));
  }
  await render();
}

async function render(): Promise<void> {
  const state = await invoke<State>('get_state', { lang });
  root.replaceChildren();

  root.append(el('h1', {}, 'Kynoko Launcher'), el('p', { class: 'lead' }, t(lang, 'LEAD')));

  // The default browser: one line, what every app follows unless it has its own.
  const browserCard = el('div', { class: 'browser-default' });
  browserCard.append(
    el('div', { class: 'field' },
      el('span', {}, t(lang, 'DEFAULT_BROWSER')),
      browserSelect(state, state.defaultBrowser, t(lang, 'SYSTEM_DEFAULT'), t(lang, 'DEFAULT_BROWSER'),
        (id) => void run(() => invoke('set_default_browser', { id }))),
      ...[profileSelect(state, state.defaultBrowser, state.defaultProfile, t(lang, 'USUAL_PROFILE'),
        (id) => void run(() => invoke('set_default_profile', { id })))].filter((x): x is HTMLSelectElement => !!x),
    ),
  );
  if (state.browsers.length < 2) browserCard.append(el('p', { class: 'note' }, t(lang, 'NO_BROWSERS')));
  // A browser that cannot give an app window (Opera ignores --app, verified;
  // Safari; Firefox outside Windows): said, with the way to get one.
  const plain = [state.defaultBrowser, ...state.apps.map((a) => a.browser)]
    .map((id) => state.browsers.find((b) => b.id === id))
    .filter((b): b is Browser => !!b && !hasAppMode(b, state.os));
  for (const b of new Set(plain)) browserCard.append(el('p', { class: 'note' }, t(lang, 'NO_APP_MODE', { browser: b.name })));
  if (state.defaultBrowser === EMBEDDED || state.apps.some((a) => a.browser === EMBEDDED)) {
    browserCard.append(el('p', { class: 'note' }, t(lang, 'EMBEDDED_HINT')));
  }
  root.append(browserCard);

  root.append(el('h2', {}, t(lang, 'APPS_TITLE')));
  const apps = el('div', { class: 'apps' });
  for (const app of state.apps) {
    // The app itself, icon and name, is what opens it.
    const open = el('button', { type: 'button', class: 'name', ariaLabel: t(lang, 'OPEN_APP', { app: app.name }) },
      iconOf(app.code),
      el('span', { class: 'label' }, el('span', {}, app.name), el('span', { class: 'open' }, t(lang, 'OPEN'))));
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
    const expand = el('button', { type: 'button', class: 'expand', ariaLabel: t(lang, 'SETTINGS_FOR', { app: app.name }) },
      // One line per setting, stacked.
      el('span', { class: 'summary' }, ...summary.map((line) => el('span', {}, line))),
      el('span', { class: expanded ? 'chevron open' : 'chevron', ariaHidden: 'true' }));
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
          browserSelect(state, app.browser, t(lang, 'FOLLOW_DEFAULT'), t(lang, 'BROWSER_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_browser', { code: app.code, id }))),
          ...[profileSelect(state, app.browser, app.profile, t(lang, 'PROFILE_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_profile', { code: app.code, id })))].filter((x): x is HTMLSelectElement => !!x)),
        expand));
    if (expanded) {
      card.append(el('div', { class: 'app-body', id: bodyId },
        // File types: only for an app that opens files.
        ...(app.types.length ? [filesOf(app)] : []),
        ...(app.shortcutItems.length ? [shortcutsOf(app, state.os)] : [])));
    }
    apps.append(card);
  }
  root.append(apps);
  if (state.apps.some((a) => a.associated)) root.append(el('p', { class: 'note' }, t(lang, 'TYPES_HINT')));

  if (state.windows) {
    const note = el('p', { class: 'note' }, t(lang, 'DEFAULT_APPS_NOTE'));
    const settings = el('button', { type: 'button' }, t(lang, 'DEFAULT_APPS_BUTTON'));
    settings.addEventListener('click', () => void run(() => invoke('open_default_apps')));
    root.append(note, el('p', {}, settings));
  }

  const remove = el('button', { type: 'button', class: 'danger' }, t(lang, 'REMOVE_ALL'));
  remove.addEventListener('click', () => {
    if (!confirm(t(lang, 'REMOVE_CONFIRM'))) return;
    void run(async () => {
      await invoke('remove_everything');
      alert(t(lang, 'REMOVED'));
    });
  });
  const date = new Date(state.catalogueDate);
  const check = el('button', { type: 'button' }, t(lang, 'CHECK_NOW'));
  check.addEventListener('click', () => void run(() => invoke('check_catalogue')));
  const status = el('span', { class: 'note' }, `Kynoko Launcher ${state.version} · ${t(lang, 'CATALOGUE', { date: date.toLocaleDateString(lang) })}`);
  const foot = el('div', { class: 'foot' }, el('span', { class: 'field' }, status, check), remove);
  root.append(foot);
  // A failure is said here, quietly, with the last success: never a notification.
  if (state.catalogueError) {
    const last = state.catalogueChecked ? new Date(state.catalogueChecked * 1000).toLocaleString(lang) : null;
    root.append(el('p', { class: 'note' }, last ? t(lang, 'CATALOGUE_FAILED', { date: last }) : t(lang, 'CATALOGUE_BUNDLED')));
  } else if (!state.catalogueChecked) {
    root.append(el('p', { class: 'note' }, t(lang, 'CATALOGUE_BUNDLED')));
  }
  if (state.focus) document.getElementById('app-' + state.focus)?.scrollIntoView({ block: 'center' });
}

// The running window was asked for again (an app's menu entry): show that app.
void listen('focus-app', () => void render());
// The catalogue changed in the background: show the new one.
void listen('catalogue-updated', () => void render());

void render();
