// Windows: the native frame and its separate menu row are replaced by one
// title bar drawn here (like VS Code): app icon, File/Edit/View/Account/Window
// menus, the title in the middle, and minimise / maximise / close.
// Menu items reuse the same actions as the native menu on macOS ('menu-action'),
// so nothing behaves differently. Clicking a menu never takes focus away from
// the editor, so Cut / Copy / Paste act on what you were editing.

import { getCurrentWindow } from '@tauri-apps/api/window';
import { bus, el } from '../state';
import { brandMarkSvg, BRAND_ORANGE } from '../icons/brandMark';
import type { MenuAction } from '../../shared/ipc';

type Item =
  | { label: string; keys?: string; action?: MenuAction; edit?: EditCommand; run?: () => void }
  | 'sep';
export type EditCommand = 'undo' | 'redo' | 'cut' | 'copy' | 'paste' | 'selectAll';

const win = (): ReturnType<typeof getCurrentWindow> => getCurrentWindow();

const MENUS: Array<{ label: string; items: Item[] }> = [
  {
    label: 'File',
    items: [
      { label: 'New Window', keys: 'Ctrl+Shift+N', action: 'new-window' },
      { label: 'Open Folder…', keys: 'Ctrl+O', action: 'open-folder' },
      { label: 'New File', action: 'new-file' },
      'sep',
      { label: 'Save', keys: 'Ctrl+S', action: 'save' },
      { label: 'Close Tab', keys: 'Ctrl+W', action: 'close-tab' },
      'sep',
      { label: 'Settings', keys: 'Ctrl+,', action: 'settings' },
      'sep',
      { label: 'Exit', run: () => void win().close() },
    ],
  },
  {
    label: 'Edit',
    items: [
      { label: 'Undo', keys: 'Ctrl+Z', edit: 'undo' },
      { label: 'Redo', keys: 'Ctrl+Y', edit: 'redo' },
      'sep',
      { label: 'Cut', keys: 'Ctrl+X', edit: 'cut' },
      { label: 'Copy', keys: 'Ctrl+C', edit: 'copy' },
      { label: 'Paste', keys: 'Ctrl+V', edit: 'paste' },
      { label: 'Select All', keys: 'Ctrl+A', edit: 'selectAll' },
    ],
  },
  {
    label: 'View',
    items: [
      { label: 'Command Palette', keys: 'Ctrl+K', action: 'command-palette' },
      'sep',
      { label: 'Toggle Sidebar', keys: 'Ctrl+B', action: 'toggle-sidebar' },
      { label: 'Toggle AI Panel', keys: 'Ctrl+J', action: 'toggle-ai-panel' },
      { label: 'Toggle Terminal', keys: 'Ctrl+`', action: 'toggle-terminal' },
      'sep',
      { label: 'Full Screen', keys: 'F11', run: () => void toggleFullscreen() },
    ],
  },
  { label: 'Account', items: [{ label: 'BlazeAPI Account', keys: 'Ctrl+Shift+A', action: 'account' }] },
  {
    label: 'Window',
    items: [
      { label: 'Minimize', run: () => void win().minimize() },
      { label: 'Maximize', run: () => void win().toggleMaximize() },
      'sep',
      { label: 'New Window', keys: 'Ctrl+Shift+N', action: 'new-window' },
    ],
  },
];

async function toggleFullscreen(): Promise<void> {
  const w = win();
  await w.setFullscreen(!(await w.isFullscreen()));
}

const SVG = (d: string): string => `<svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1">${d}</svg>`;
const ICON_MIN = SVG('<path d="M0 5.5h10"/>');
const ICON_MAX = SVG('<rect x="0.5" y="0.5" width="9" height="9"/>');
const ICON_RESTORE = SVG('<rect x="0.5" y="2.5" width="7" height="7"/><path d="M2.5 2.5V0.5h7v7h-2"/>');
const ICON_CLOSE = SVG('<path d="M0.5 0.5l9 9M9.5 0.5l-9 9"/>');

export function initWindowsTitlebar(titlebar: HTMLElement, titleText: HTMLElement, settingsBtn: HTMLElement): void {
  titlebar.classList.add('win-tb');
  const left = el('div', 'tb-left');
  const logo = el('div', 'tb-logo', brandMarkSvg(16, BRAND_ORANGE, 2));
  const bar = el('div', 'tb-menus');
  bar.setAttribute('role', 'menubar');
  left.append(logo, bar);

  const right = el('div', 'tb-right');
  const minBtn = el('button', 'tb-win', ICON_MIN);
  minBtn.title = 'Minimize';
  const maxBtn = el('button', 'tb-win', ICON_MAX);
  maxBtn.title = 'Maximize';
  const closeBtn = el('button', 'tb-win tb-close', ICON_CLOSE);
  closeBtn.title = 'Close';
  settingsBtn.classList.add('tb-gear');
  right.append(settingsBtn, minBtn, maxBtn, closeBtn);
  titlebar.prepend(left);
  titlebar.append(right);
  // Tauri drags only from the marked element itself, not its children.
  for (const d of [left, logo, bar, right]) d.setAttribute('data-tauri-drag-region', '');

  for (const b of [minBtn, maxBtn, closeBtn]) b.addEventListener('mousedown', (e) => e.stopPropagation());
  minBtn.addEventListener('click', () => void win().minimize());
  maxBtn.addEventListener('click', () => void win().toggleMaximize());
  closeBtn.addEventListener('click', () => void win().close());
  const syncMax = async (): Promise<void> => {
    try {
      const m = await win().isMaximized();
      maxBtn.innerHTML = m ? ICON_RESTORE : ICON_MAX;
      maxBtn.title = m ? 'Restore' : 'Maximize';
      document.documentElement.classList.toggle('win-maximized', m);
    } catch { /* not in Tauri (tests) */ }
  };
  void syncMax();
  void win().onResized(() => void syncMax()).catch(() => undefined);

  // ---- menus ----
  let open: { index: number; panel: HTMLElement } | null = null;
  const tops: HTMLElement[] = [];

  function close(): void {
    if (!open) return;
    open.panel.remove();
    tops[open.index].classList.remove('open');
    open = null;
  }

  function run(it: Exclude<Item, 'sep'>): void {
    close();
    if (it.action) bus.emit('menu-action', it.action);
    else if (it.edit) bus.emit('edit-command', it.edit);
    else it.run?.();
  }

  function show(index: number): void {
    close();
    const top = tops[index];
    const panel = el('div', 'tb-menu');
    panel.setAttribute('role', 'menu');
    for (const it of MENUS[index].items) {
      if (it === 'sep') { panel.appendChild(el('div', 'tb-sep')); continue; }
      const row = el('button', 'tb-item');
      row.setAttribute('role', 'menuitem');
      row.appendChild(el('span', '', it.label));
      if (it.keys) row.appendChild(el('span', 'tb-keys', it.keys));
      row.addEventListener('mousedown', (e) => e.preventDefault());
      row.addEventListener('click', () => run(it));
      panel.appendChild(row);
    }
    const r = top.getBoundingClientRect();
    panel.style.left = `${Math.round(r.left)}px`;
    panel.style.top = `${Math.round(r.bottom + 2)}px`;
    (document.getElementById('overlay-root') ?? document.body).appendChild(panel);
    top.classList.add('open');
    open = { index, panel };
  }

  MENUS.forEach((m, i) => {
    const b = el('button', 'tb-top', m.label);
    b.setAttribute('role', 'menuitem');
    // Keep focus where it is (the editor) so Edit > Copy etc. act on it.
    b.addEventListener('mousedown', (e) => { e.preventDefault(); e.stopPropagation(); if (open?.index === i) close(); else show(i); });
    b.addEventListener('mouseenter', () => { if (open && open.index !== i) show(i); });
    tops.push(b);
    bar.appendChild(b);
  });

  document.addEventListener('mousedown', (e) => {
    if (open && !open.panel.contains(e.target as Node)) close();
  }, true);
  window.addEventListener('blur', close);
  window.addEventListener('resize', close);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && open) { e.preventDefault(); close(); return; }
    // Shortcuts the native menu used to provide.
    if (e.ctrlKey && e.shiftKey && !e.altKey && e.key.toLowerCase() === 'n') { e.preventDefault(); bus.emit('menu-action', 'new-window'); }
    if (e.key === 'F11') { e.preventDefault(); void toggleFullscreen(); }
  });

  titleText.classList.add('tb-title');
}

/** Edit menu commands: Monaco gets its own commands, other fields the browser's. */
export function runEditCommand(cmd: EditCommand, monacoRun: ((id: string) => boolean) | null): void {
  const ids: Record<EditCommand, string> = {
    undo: 'undo', redo: 'redo', cut: 'editor.action.clipboardCutAction', copy: 'editor.action.clipboardCopyAction',
    paste: 'editor.action.clipboardPasteAction', selectAll: 'editor.action.selectAll',
  };
  if (monacoRun?.(ids[cmd])) return;
  if (cmd === 'paste') {
    void navigator.clipboard.readText().then((t) => document.execCommand('insertText', false, t), () => undefined);
    return;
  }
  document.execCommand(cmd);
}
