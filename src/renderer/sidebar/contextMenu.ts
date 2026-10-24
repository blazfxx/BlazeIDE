// Context menu rendered into #overlay-root. Closes on click-away or Esc.

import { el } from '../state';

export interface CtxItem {
  label: string;
  danger?: boolean;
  action(): void;
}

let openMenu: HTMLElement | null = null;

export function closeContextMenu(): void {
  openMenu?.remove();
  openMenu = null;
  document.removeEventListener('mousedown', onAway, true);
  document.removeEventListener('keydown', onKey, true);
}

function onAway(e: MouseEvent): void {
  if (openMenu && !openMenu.contains(e.target as Node)) closeContextMenu();
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') closeContextMenu();
}

export function showContextMenu(x: number, y: number, items: CtxItem[]): void {
  closeContextMenu();
  const overlay = document.getElementById('overlay-root');
  if (!overlay) return;
  const menu = el('div', 'ctx-menu');
  for (const item of items) {
    const row = el('div', `ctx-item${item.danger ? ' danger' : ''}`);
    row.textContent = item.label;
    row.addEventListener('click', () => {
      closeContextMenu();
      item.action();
    });
    menu.appendChild(row);
  }
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  overlay.appendChild(menu);
  // Keep on screen
  const rect = menu.getBoundingClientRect();
  if (rect.right > window.innerWidth) menu.style.left = `${x - rect.width}px`;
  if (rect.bottom > window.innerHeight) menu.style.top = `${y - rect.height}px`;
  openMenu = menu;
  document.addEventListener('mousedown', onAway, true);
  document.addEventListener('keydown', onKey, true);
}
