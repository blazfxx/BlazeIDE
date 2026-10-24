// The three editor-area surfaces: Monaco mount, custom-tab host, empty state.

import { formatChord } from '../keybindings';
import { el } from '../state';

export interface Surfaces {
  editorEl: HTMLElement;
  customHost: HTMLElement;
  show(kind: 'editor' | 'custom' | 'empty'): void;
}

export function createSurfaces(hostEl: HTMLElement): Surfaces {
  const editorEl = el('div', 'editor-mount');
  editorEl.style.cssText = 'position:absolute;inset:0;display:none;';
  const customHost = el('div', 'custom-tab-host');
  customHost.style.cssText = 'position:absolute;inset:0;display:none;overflow:auto;';
  const empty = el(
    'div',
    'editor-empty',
    `<div class="empty-mark"></div><div>Open a folder to start</div><div class="empty-hint">${formatChord('Cmd+O')}</div>`
  );
  hostEl.append(editorEl, customHost, empty);
  return {
    editorEl,
    customHost,
    show(kind) {
      editorEl.style.display = kind === 'editor' ? 'block' : 'none';
      customHost.style.display = kind === 'custom' ? 'block' : 'none';
      empty.style.display = kind === 'empty' ? 'flex' : 'none';
    },
  };
}
