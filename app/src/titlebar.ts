import { defaultWindowIcon } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { el, icon } from './dom';

/**
 * The settings window's own title bar, as a Kynoko window's (the skeleton's
 * KynokoNativeWindowService): its empty parts move the window, a double
 * press maximizes it, and it carries the window's buttons at its end. macOS
 * keeps its own buttons, drawn over the bar's start (tauri.conf.json:
 * titleBarStyle "Overlay"), on the left whatever the page's language: they
 * follow the system. Windows and Linux lose the system's bar when the
 * launcher starts (lib.rs, setup).
 *
 * Through the launcher's own_window_* commands, which act on the window the
 * page is in and name no other (capability "default").
 */
export const MACOS = /Macintosh/.test(navigator.userAgent);

type WindowCommand = 'drag' | 'minimize' | 'toggle_maximize' | 'is_maximized' | 'close';

function call(cmd: WindowCommand): Promise<unknown> {
  return invoke(`own_window_${cmd}`).catch(() => null);
}

/** What in the bar is NOT a place to grab the window by. */
const INTERACTIVE = 'button, a, input, select, textarea, label, summary, [role="button"], [role="dialog"], [role="tooltip"], [tabindex]';

/** The window's buttons' names, in the user's language. */
export interface WindowLabels { minimize: string; maximize: string; restore: string; close: string }

/** Windows' own glyphs: 1 px lines in a 10 px square. */
const GLYPHS = {
  minimize: ['M0 5.5h10'],
  maximize: ['M.5 .5h9v9h-9z'],
  restore: ['M.5 2.5h7v7h-7z', 'M2.5 2.5V.5h7v7h-2'],
  close: ['M.5 .5l9 9M9.5 .5l-9 9'],
};

function glyph(paths: readonly string[]): SVGSVGElement {
  const svg = icon(paths, 10, 10, 1);
  svg.setAttribute('stroke-linecap', 'square');
  svg.setAttribute('stroke-linejoin', 'miter');
  svg.setAttribute('shape-rendering', 'crispEdges');
  return svg;
}

/**
 * The bar, around what the page puts in it (`parts`: the name, the account),
 * with the window's buttons at the end (not on macOS).
 */
export function titleBar(labels: WindowLabels, ...parts: HTMLElement[]): HTMLElement {
  const bar = el('header', { class: MACOS ? 'titlebar macos' : 'titlebar' }, ...parts);
  let refresh = (): void => {};
  bar.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || !(e.target instanceof Element) || e.target.closest(INTERACTIVE)) return;
    e.preventDefault();
    if (e.detail === 2) void call('toggle_maximize').then(() => refresh());
    else void call('drag');
  });
  if (!MACOS) {
    const button = (cls: string, label: string, paths: readonly string[], action: () => void) => {
      const b = el('button', { type: 'button', class: `win-btn ${cls}`, ariaLabel: label, tabIndex: -1 }, glyph(paths));
      b.addEventListener('click', action);
      return b;
    };
    const max = button('max', labels.maximize, GLYPHS.maximize, () => void call('toggle_maximize').then(() => refresh()));
    refresh = () => {
      void call('is_maximized').then((maximized) => {
        const on = maximized === true;
        max.replaceChildren(glyph(on ? GLYPHS.restore : GLYPHS.maximize));
        max.ariaLabel = on ? labels.restore : labels.maximize;
      });
    };
    bar.append(el('div', { class: 'win-buttons' },
      button('min', labels.minimize, GLYPHS.minimize, () => void call('minimize')),
      max,
      // Closing the settings window hides it (lib.rs): the launcher stays
      // while a file or an assistant needs it, then quits by itself.
      button('close', labels.close, GLYPHS.close, () => void call('close'))));
    // A snap, the system's shortcuts, a double press: no event says it, a
    // resize does.
    let pending = 0;
    window.addEventListener('resize', () => {
      clearTimeout(pending);
      pending = window.setTimeout(refresh, 120);
    });
    refresh();
  }
  // The bar fades with the window when another one has the focus, as the
  // system's own does.
  window.addEventListener('blur', () => bar.classList.add('inactive'));
  window.addEventListener('focus', () => bar.classList.remove('inactive'));
  return bar;
}

let iconUrl: Promise<string | null> | null = null;

/**
 * The launcher's own icon (the one its taskbar button wears), as a picture
 * the page can show: the repository carries no brand asset (TRADEMARKS.md),
 * the icon is the build's.
 */
export function windowIcon(): Promise<string | null> {
  iconUrl ??= (async () => {
    const image = await defaultWindowIcon();
    if (!image) return null;
    try {
      const [{ width, height }, rgba] = await Promise.all([image.size(), image.rgba()]);
      const canvas = el('canvas', { width, height });
      canvas.getContext('2d')?.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
      return canvas.toDataURL('image/png');
    } finally {
      void image.close();
    }
  })().catch(() => null);
  return iconUrl;
}

/** An <img> of the launcher's icon, shown once it is there. */
export function windowIconImg(cls: string, size: number): HTMLImageElement {
  const img = el('img', { class: cls, alt: '', width: size, height: size });
  img.hidden = true;
  void windowIcon().then((src) => {
    if (src) {
      img.src = src;
      img.hidden = false;
    }
  });
  return img;
}
