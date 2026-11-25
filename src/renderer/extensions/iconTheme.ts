// File icon themes from extensions (VS Code `iconThemes`, picture-based ones,
// e.g. Material Icon Theme). The tree asks `themedIcon` first and falls back
// to Blaze's own icons when no theme is chosen or nothing matches.

import type { IconThemeData } from '../../shared/types';
import { bus, state } from '../state';
import { languageForPath } from '../editor/languages';

type Dict = Record<string, string>;
interface Map_ {
  file?: string; folder?: string; folderExpanded?: string;
  fileNames?: Dict; fileExtensions?: Dict; folderNames?: Dict; folderNamesExpanded?: Dict; languageIds?: Dict;
  light?: Omit<Map_, 'light'>;
}

let data: IconThemeData | null = null;
let loadedKey = '';
let seq = 0;

function dict(v: unknown): Dict {
  return v && typeof v === 'object' ? (v as Dict) : {};
}

/** The mappings for the current light/dark look (VS Code's `light` section overrides). */
function view(): Required<Omit<Map_, 'light'>> {
  const m = (data?.map ?? {}) as Map_;
  const l = state.settings.theme === 'light' && m.light ? m.light : {};
  const pick = (k: 'fileNames' | 'fileExtensions' | 'folderNames' | 'folderNamesExpanded' | 'languageIds'): Dict => ({ ...dict(m[k]), ...dict(l[k]) });
  return {
    file: l.file ?? m.file ?? '', folder: l.folder ?? m.folder ?? '', folderExpanded: l.folderExpanded ?? m.folderExpanded ?? m.folder ?? '',
    fileNames: pick('fileNames'), fileExtensions: pick('fileExtensions'), folderNames: pick('folderNames'), folderNamesExpanded: pick('folderNamesExpanded'), languageIds: pick('languageIds'),
  };
}

let cache: { key: string; v: ReturnType<typeof view> } | null = null;
function current(): ReturnType<typeof view> | null {
  if (!data) return null;
  const key = `${loadedKey}|${state.settings.theme}`;
  if (!cache || cache.key !== key) cache = { key, v: view() };
  return cache.v;
}

function img(id: string | undefined): string | null {
  const url = id ? data?.icons[id] : undefined;
  return url ? `<img class="ti" src="${url}" alt="" draggable="false">` : null;
}

/** Icon definition id for a file, by VS Code's precedence. */
export function fileIconId(name: string, v = current()): string | undefined {
  if (!v) return undefined;
  const lower = name.toLowerCase();
  if (v.fileNames[lower] || v.fileNames[name]) return v.fileNames[lower] ?? v.fileNames[name];
  const parts = lower.split('.');
  for (let i = 1; i < parts.length; i++) {
    const ext = parts.slice(i).join('.');
    if (v.fileExtensions[ext]) return v.fileExtensions[ext];
  }
  const lang = languageForPath(name);
  if (lang && v.languageIds[lang]) return v.languageIds[lang];
  return v.file || undefined;
}

export function folderIconId(name: string, open: boolean, v = current()): string | undefined {
  if (!v) return undefined;
  const lower = name.toLowerCase();
  const named = open ? v.folderNamesExpanded[lower] ?? v.folderNames[lower] : v.folderNames[lower];
  return named ?? ((open ? v.folderExpanded : v.folder) || undefined);
}

/** `<img>` markup for a file or folder from the active icon theme, or null. */
export function themedIcon(name: string, folder: boolean, open = false): string | null {
  if (!data) return null;
  return img(folder ? folderIconId(name, open) : fileIconId(name));
}

export function activeIconTheme(): string {
  return data ? loadedKey : '';
}

/** Load the icon theme named in settings (or drop it); emits 'icon-theme-changed'. */
export async function syncIconTheme(): Promise<void> {
  const key = state.settings.iconTheme || '';
  if (key === loadedKey && (key === '' || data)) return;
  const mine = ++seq;
  const slash = key.indexOf('/');
  let next: IconThemeData | null = null;
  if (key && slash > 0) {
    next = await window.void.extIconTheme(key.slice(0, slash), key.slice(slash + 1)).catch(() => null);
  }
  if (mine !== seq) return;
  data = next;
  loadedKey = next ? key : '';
  cache = null;
  bus.emit('icon-theme-changed', undefined);
}

export function initIconThemes(): void {
  bus.on('settings-changed', () => { cache = null; void syncIconTheme(); });
  bus.on('extensions-changed', () => { loadedKey = '__reload__'; data = null; void syncIconTheme(); });
  void syncIconTheme();
}
