/** Builds an element; text goes through textContent, never innerHTML. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<HTMLElementTagNameMap[K]> & { class?: string } = {},
  ...children: (Node | string | null)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  const { class: cls, ...rest } = props;
  if (cls) node.className = cls;
  Object.assign(node, rest);
  node.append(...children.filter((c): c is Node | string => c !== null));
  return node;
}

/**
 * Names a control for the repaint: the window is drawn anew after each
 * change, and the control that had the focus gets it back (main.ts, render).
 */
export function keyed<T extends HTMLElement>(node: T, key: string): T {
  node.dataset.key = key;
  return node;
}

const SVG = 'http://www.w3.org/2000/svg';

/**
 * A line drawing in the text's colour, decorative (the control around it
 * carries the words): paths in a `box` unit square, drawn `size` pixels wide.
 */
export function icon(paths: readonly string[], size = 16, box = 16, stroke = 1.6): SVGSVGElement {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', `0 0 ${box} ${box}`);
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', String(stroke));
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('aria-hidden', 'true');
  svg.classList.add('icon');
  for (const d of paths) {
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    svg.append(path);
  }
  return svg;
}

/** The drawings the window uses (a 16 unit square). */
export const ICONS = {
  person: ['M8 8.2a2.9 2.9 0 1 0 0-5.8 2.9 2.9 0 0 0 0 5.8Z', 'M2.6 14c.7-2.5 2.8-3.9 5.4-3.9s4.7 1.4 5.4 3.9'],
  chevronDown: ['M4.5 6.5 8 10l3.5-3.5'],
  refresh: ['M13.2 8.6A5.3 5.3 0 1 1 11.6 4', 'M12.2 1.8v2.8H9.4'],
  close: ['M4.5 4.5l7 7M11.5 4.5l-7 7'],
  check: ['M3.6 8.4l2.9 2.9 5.9-6.4'],
  signOut: ['M6.5 2.8H3.8a1 1 0 0 0-1 1v8.4a1 1 0 0 0 1 1h2.7', 'M10.2 5.2 13 8l-2.8 2.8', 'M12.8 8H6.4'],
  apps: ['M2.8 2.8h4v4h-4zM9.2 2.8h4v4h-4zM2.8 9.2h4v4h-4zM9.2 9.2h4v4h-4z'],
  sparkle: ['M8 2.2c.4 2.9 1.9 4.4 4.8 4.8-2.9.4-4.4 1.9-4.8 4.8-.4-2.9-1.9-4.4-4.8-4.8 2.9-.4 4.4-1.9 4.8-4.8Z', 'M12.6 10.6c.2 1.1.7 1.6 1.8 1.8-1.1.2-1.6.7-1.8 1.8-.2-1.1-.7-1.6-1.8-1.8 1.1-.2 1.6-.7 1.8-1.8Z'],
  gear: [
    'M8 10.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4Z',
    'M8 1.8v1.6M8 12.6v1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M1.8 8h1.6M12.6 8h1.6M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1',
  ],
  info: ['M8 14a6 6 0 1 0 0-12 6 6 0 0 0 0 12Z', 'M8 7.4v3.6', 'M8 5.1v.1'],
} as const;
