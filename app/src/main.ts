import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Lang, pickLang, t } from './i18n';

interface Profile { id: string; name: string; default: boolean }
interface Browser { id: string; name: string; engine: string; profiles: Profile[] }
interface AppView { code: string; name: string; extensions: string[]; associated: boolean; shortcuts: boolean; browser: string | null; profile: string | null }
interface State {
  apps: AppView[];
  browsers: Browser[];
  defaultBrowser: string | null;
  defaultProfile: string | null;
  catalogueDate: string;
  windows: boolean;
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

function browserSelect(state: State, value: string | null, first: string, label: string, onChange: (id: string | null) => void) {
  const select = el('select', { ariaLabel: label });
  select.append(el('option', { value: '' }, first));
  for (const b of state.browsers) select.append(el('option', { value: b.id, selected: b.id === value }, b.name));
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
  if (!state.browsers.length) browserCard.append(el('p', { class: 'note' }, t(lang, 'NO_BROWSERS')));
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
        el('span', { class: 'name' }, app.name),
        el('span', { class: 'controls' },
          browserSelect(state, app.browser, t(lang, 'FOLLOW_DEFAULT'), t(lang, 'BROWSER_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_browser', { code: app.code, id }))),
          ...[profileSelect(state, app.browser, app.profile, t(lang, 'PROFILE_FOR', { app: app.name }),
            (id) => void run(() => invoke('set_app_profile', { code: app.code, id })))].filter((x): x is HTMLSelectElement => !!x),
          el('label', { class: 'switch' }, toggle, t(lang, 'OPEN_FILES')),
          el('label', { class: 'switch' }, shortcuts, t(lang, 'SHORTCUTS')),
          open,
        ),
        // Extensions are technical: left to right whatever the language.
        el('span', { class: 'exts', dir: 'ltr' }, app.extensions.map((x) => '.' + x).join('  ')),
      ),
    );
  }
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
  root.append(
    el('div', { class: 'foot' },
      el('span', { class: 'note' }, t(lang, 'CATALOGUE', { date: date.toLocaleDateString(lang) })),
      remove,
    ),
  );
  if (state.focus) document.getElementById('app-' + state.focus)?.scrollIntoView({ block: 'center' });
}

// The running window was asked for again (an app's menu entry): show that app.
void listen('focus-app', () => void render());

void render();
