import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Lang, pickLang, t } from './i18n';

interface Profile { id: string; name: string; default: boolean }
interface Browser { id: string; name: string; engine: string; profiles: Profile[] }
interface TypeGroup { facade: string; exts: { ext: string; on: boolean }[] }
interface AppView { code: string; name: string; types: TypeGroup[]; associated: boolean; shortcuts: boolean; browser: string | null; profile: string | null }
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

/**
 * An app's file types, by the facade each opens in, one tick per type. They
 * can be chosen before the app is associated: the switch then applies them.
 */
function typesOf(app: AppView): HTMLElement {
  const box = el('div', { class: app.associated ? 'types' : 'types idle', role: 'group', ariaLabel: t(lang, 'TYPES_FOR', { app: app.name }) });
  for (const group of app.types) {
    // Extensions are technical: left to right whatever the language.
    const exts = el('span', { class: 'chips', dir: 'ltr' });
    for (const x of group.exts) {
      const tick = el('input', { type: 'checkbox', checked: x.on });
      tick.addEventListener('change', () => void run(() => invoke('set_extension', { code: app.code, ext: x.ext, on: tick.checked })));
      exts.append(el('label', { class: 'chip' }, tick, '.' + x.ext));
    }
    box.append(el('div', { class: 'type-group' }, el('span', { class: 'facade' }, group.facade), exts));
  }
  const all = app.types.flatMap((g) => g.exts);
  if (all.length > 1) {
    const bulk = (label: string, on: boolean) => {
      const b = el('button', { type: 'button', class: 'link', disabled: all.every((x) => x.on === on) }, label);
      b.addEventListener('click', () => void run(() => invoke('set_extension', { code: app.code, ext: null, on })));
      return b;
    };
    box.append(el('div', { class: 'bulk' }, bulk(t(lang, 'ALL_TYPES'), true), bulk(t(lang, 'NO_TYPES'), false)));
  }
  return box;
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

  root.append(el('h2', {}, t(lang, 'BROWSER_TITLE')));
  const browserCard = el('div', { class: 'card' });
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
  const appsCard = el('div', { class: 'card' });
  for (const app of state.apps) {
    const toggle = el('input', { type: 'checkbox', checked: app.associated });
    toggle.addEventListener('change', () => void run(() => invoke('set_associated', { code: app.code, on: toggle.checked })));
    const shortcuts = el('input', { type: 'checkbox', checked: app.shortcuts });
    shortcuts.addEventListener('change', () => void run(() => invoke('set_shortcuts', { code: app.code, on: shortcuts.checked, lang })));
    const open = el('button', { type: 'button' }, t(lang, 'OPEN_APP'));
    open.addEventListener('click', () => void run(() => invoke('launch', { target: app.code })));
    appsCard.append(
      el('div', { class: app.code === state.focus ? 'row focus' : 'row', id: 'app-' + app.code },
        el('span', { class: 'name' }, iconOf(app.code), app.name),
        el('span', { class: 'controls' },
          browserSelect(state, app.browser, t(lang, 'FOLLOW_DEFAULT'), t(lang, 'BROWSER_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_browser', { code: app.code, id }))),
          ...[profileSelect(state, app.browser, app.profile, t(lang, 'PROFILE_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_profile', { code: app.code, id })))].filter((x): x is HTMLSelectElement => !!x),
          el('label', { class: 'switch' }, toggle, t(lang, 'OPEN_FILES')),
          el('label', { class: 'switch' }, shortcuts, t(lang, state.os === 'macos' ? 'SHORTCUTS_MACOS' : state.os === 'linux' ? 'SHORTCUTS_LINUX' : 'SHORTCUTS_WINDOWS')),
          open,
        ),
        typesOf(app),
      ),
    );
  }
  if (state.apps.some((a) => a.types.length)) appsCard.append(el('p', { class: 'note' }, t(lang, 'TYPES_HINT')));
  root.append(appsCard);

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
