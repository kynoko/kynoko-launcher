import { ICONS, el, icon } from './dom';

/**
 * What floats over the window, drawn like the rest of it rather than by the
 * web engine (whose alert() and confirm() are titled with the page's address
 * and look like no Kynoko window): a question, a passing message, a tooltip.
 */

/**
 * Asks a question in a modal dialog: true when `ok` is pressed, false on
 * `cancel`, Escape or closing. The paragraphs of `lines` are its text.
 */
export function confirmDialog(lines: readonly string[], ok: string, cancel: string, opts: { danger?: boolean } = {}): Promise<boolean> {
  return new Promise((resolve) => {
    const yes = el('button', { type: 'button', class: opts.danger ? 'danger-solid' : 'primary' }, ok);
    const no = el('button', { type: 'button' }, cancel);
    const dialog = el('dialog', { class: 'k-dialog' },
      el('div', { class: 'dialog-text' }, ...lines.map((line) => el('p', {}, line))),
      el('div', { class: 'dialog-actions' }, no, yes));
    let answer = false;
    yes.addEventListener('click', () => {
      answer = true;
      dialog.close();
    });
    no.addEventListener('click', () => dialog.close());
    dialog.addEventListener('close', () => {
      dialog.remove();
      resolve(answer);
    });
    document.body.append(dialog);
    dialog.showModal();
    // A question that destroys something starts on the way out of it.
    (opts.danger ? no : yes).focus();
  });
}

let shelf: HTMLElement | null = null;

/**
 * A passing message at the bottom of the window (a confirmation, a failure):
 * read out by screen readers, gone after a moment or with its cross.
 */
export function toast(text: string, kind: 'info' | 'error', closeLabel: string): void {
  if (!shelf) {
    shelf = el('div', { class: 'toasts' });
    document.body.append(shelf);
  }
  const close = el('button', { type: 'button', class: 'toast-close', ariaLabel: closeLabel }, icon(ICONS.close, 14));
  const note = el('div', { class: `toast ${kind}`, role: kind === 'error' ? 'alert' : 'status' },
    icon(kind === 'error' ? ICONS.info : ICONS.check, 16),
    el('span', { class: 'toast-text' }, text),
    close);
  const remove = () => {
    note.classList.add('leaving');
    window.setTimeout(() => note.remove(), 180);
  };
  close.addEventListener('click', remove);
  shelf.append(note);
  // An error stays long enough to be read whole.
  window.setTimeout(remove, kind === 'error' ? 12_000 : 4_000);
}

let tips = 0;

/**
 * A tooltip under `target` (no `title` attribute: the system's tooltip
 * ignores the theme): after half a second of hovering, at once for the
 * keyboard, gone a tenth of a second after (Windows' delays). Also its
 * description for screen readers. `host` is the positioned box it sits in.
 */
export function tooltip(target: HTMLElement, text: string, host: HTMLElement): void {
  const tip = el('div', { class: 'tip', role: 'tooltip', id: `tip-${++tips}` }, text);
  target.setAttribute('aria-describedby', tip.id);
  host.append(tip);
  let timer = 0;
  const show = (delay: number) => {
    clearTimeout(timer);
    timer = window.setTimeout(() => tip.classList.add('on'), delay);
  };
  const hide = (delay: number) => {
    clearTimeout(timer);
    timer = window.setTimeout(() => tip.classList.remove('on'), delay);
  };
  target.addEventListener('pointerenter', () => show(500));
  target.addEventListener('pointerleave', () => hide(100));
  target.addEventListener('pointerdown', () => hide(0));
  target.addEventListener('focus', () => {
    if (target.matches(':focus-visible')) show(0);
  });
  target.addEventListener('blur', () => hide(0));
}
