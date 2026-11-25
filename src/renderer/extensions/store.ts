// Shared extension state for the sidebar list and the details tabs: the
// installed list, installs in progress, and install/uninstall that keep both
// views in step (via 'extensions-changed').

import type { InstalledExt } from '../../shared/types';
import { bus, state } from '../state';
import { apply } from '../settings/controls';
import { themeKey } from './apply';

let installed: InstalledExt[] = [];
let loaded: Promise<void> | null = null;
const busy = new Set<string>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const f of listeners) f();
}

export function onExtChange(f: () => void): () => void {
  listeners.add(f);
  return () => listeners.delete(f);
}

export async function refreshInstalled(): Promise<InstalledExt[]> {
  installed = await window.void.extList().catch(() => [] as InstalledExt[]);
  notify();
  return installed;
}

export function installedList(): InstalledExt[] {
  return installed;
}

export function ensureLoaded(): Promise<void> {
  if (!loaded) loaded = refreshInstalled().then(() => undefined);
  return loaded;
}

export function findInstalled(id: string): InstalledExt | undefined {
  return installed.find((e) => e.id === id);
}

export function isBusy(id: string): boolean {
  return busy.has(id);
}

export function compact(n: number): string {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
}

export function summary(e: InstalledExt): string {
  const parts: string[] = [];
  if (e.themes.length) parts.push(`${e.themes.length} theme${e.themes.length === 1 ? '' : 's'}`);
  if (e.snippets.length) parts.push(`${e.snippets.length} snippet set${e.snippets.length === 1 ? '' : 's'}`);
  const icons = e.iconThemes?.length ?? 0;
  if (icons) parts.push(`${icons} file icon theme${icons === 1 ? '' : 's'}`);
  return parts.length ? `Provides ${parts.join(' and ')}` : 'Nothing in this extension can run in Blaze IDE';
}

export async function installExt(namespace: string, name: string): Promise<InstalledExt> {
  const id = `${namespace}.${name}`;
  busy.add(id);
  notify();
  try {
    const ext = await window.void.extInstall(namespace, name);
    bus.emit('extensions-changed', undefined);
    await refreshInstalled();
    return ext;
  } finally {
    busy.delete(id);
    notify();
  }
}

export async function uninstallExt(id: string): Promise<void> {
  const e = findInstalled(id);
  await window.void.extUninstall(id);
  if (e && e.themes.some((t) => themeKey(e.id, t.label) === state.settings.editorTheme)) await apply({ editorTheme: '' });
  if (state.settings.iconTheme.startsWith(`${id}/`)) await apply({ iconTheme: '' });
  bus.emit('extensions-changed', undefined);
  await refreshInstalled();
}

/** Use one of an installed extension's file icon themes in the file tree. */
export async function useIconTheme(extId: string, themeId: string): Promise<void> {
  await apply({ iconTheme: `${extId}/${themeId}` });
  notify();
}

/** Use one of an installed extension's colour themes in the editor. */
export async function useTheme(extId: string, label: string): Promise<void> {
  await apply({ editorTheme: themeKey(extId, label) });
  notify();
}
