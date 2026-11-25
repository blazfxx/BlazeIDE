// An extension colour theme recolours the whole window, not just the editor:
// VS Code's workbench colours (sideBar.background, list.hoverBackground,
// button.background, ...) are mapped onto Blaze's CSS variables. Turned off
// by Settings > Appearance > "Colour the whole window", or by picking the
// Blaze default theme. A custom background keeps working: panels are drawn
// from --bg-panel at partial opacity either way.

import type { AppSettings, ExtContributions } from '../../shared/types';
import { bus, state } from '../state';
import { themeKey } from './apply';

type RGBA = [number, number, number, number];
const VARS = ['--bg-base', '--bg-panel', '--bg-elevated', '--bg-hover', '--bg-active', '--bg-card', '--border', '--border-soft', '--text-primary', '--text-secondary', '--text-disabled', '--accent', '--accent-hover', '--text-on-accent', '--focus-ring', '--ring'];

function parse(hex: unknown): RGBA | null {
  if (typeof hex !== 'string') return null;
  const m = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(hex.trim());
  if (!m) return null;
  let h = m[1];
  if (h.length <= 4) h = [...h].map((c) => c + c).join('');
  const n = (i: number): number => parseInt(h.slice(i, i + 2), 16);
  return [n(0), n(2), n(4), h.length === 8 ? n(6) / 255 : 1];
}
/** `top` drawn over an opaque `under`. */
function over(top: RGBA, under: RGBA): RGBA {
  const a = top[3];
  return [top[0] * a + under[0] * (1 - a), top[1] * a + under[1] * (1 - a), top[2] * a + under[2] * (1 - a), 1];
}
function mix(a: RGBA, b: RGBA, t: number): RGBA {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1];
}
const css = (c: RGBA, alpha = 1): string => `rgba(${Math.round(c[0])}, ${Math.round(c[1])}, ${Math.round(c[2])}, ${alpha})`;
function lum(c: RGBA): number {
  const f = (v: number): number => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4); };
  return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
}

/** CSS variable values for a VS Code theme, or null if it has no usable colours. */
export function workbenchVars(json: Record<string, unknown>, base: string): Record<string, string> | null {
  const colors = (json.colors as Record<string, unknown>) ?? {};
  const get = (...keys: string[]): RGBA | null => {
    for (const k of keys) { const c = parse(colors[k]); if (c) return c; }
    return null;
  };
  // Theme-wide defaults can also live in an unscoped tokenColors entry.
  let tokenBg: RGBA | null = null, tokenFg: RGBA | null = null;
  for (const tc of (Array.isArray(json.tokenColors) ? json.tokenColors : []) as Array<{ scope?: unknown; settings?: { background?: string; foreground?: string } }>) {
    if (!tc?.scope && tc?.settings) { tokenBg = tokenBg ?? parse(tc.settings.background); tokenFg = tokenFg ?? parse(tc.settings.foreground); }
  }
  const light = base === 'vs';
  const bg0 = get('editor.background') ?? tokenBg;
  if (!bg0) return null;
  const bg = over(bg0, light ? [255, 255, 255, 1] : [0, 0, 0, 1]);
  const fg = over(get('foreground', 'editor.foreground', 'sideBar.foreground') ?? tokenFg ?? (light ? [30, 30, 30, 1] : [230, 230, 230, 1]), bg);
  const lift = (t: number): RGBA => mix(bg, fg, t);
  const solid = (c: RGBA | null, fallback: RGBA): RGBA => (c ? over(c, bg) : fallback);

  const panel = solid(get('sideBar.background', 'editorGroupHeader.tabsBackground', 'activityBar.background'), mix(bg, light ? [0, 0, 0, 1] : [0, 0, 0, 1], light ? 0.03 : 0.18));
  const elevated = solid(get('input.background', 'dropdown.background', 'editorWidget.background'), lift(0.06));
  const hover = solid(get('list.hoverBackground', 'toolbar.hoverBackground'), lift(0.09));
  const active = solid(get('list.activeSelectionBackground', 'list.inactiveSelectionBackground', 'editor.selectionBackground'), lift(0.15));
  const border = solid(get('panel.border', 'sideBar.border', 'editorGroup.border', 'contrastBorder', 'widget.border'), lift(0.16));
  let accent = get('button.background', 'focusBorder', 'textLink.foreground', 'activityBarBadge.background');
  accent = accent ? over(accent, bg) : fg;
  const accentHover = solid(get('button.hoverBackground'), mix(accent, fg, 0.18));
  const onAccent = parse(colors['button.foreground']) ?? (lum(accent) > 0.45 ? [17, 17, 17, 1] : [255, 255, 255, 1]);
  const secondary = solid(get('descriptionForeground', 'sideBarSectionHeader.foreground', 'tab.inactiveForeground'), mix(fg, bg, 0.42));

  return {
    '--bg-base': css(bg),
    '--bg-panel': css(panel),
    '--bg-elevated': css(elevated),
    '--bg-hover': css(hover),
    '--bg-active': css(active),
    '--bg-card': css(mix(bg, panel, 0.5)),
    '--border': css(border),
    '--border-soft': css(fg, light ? 0.09 : 0.08),
    '--text-primary': css(fg),
    '--text-secondary': css(secondary),
    '--text-disabled': css(mix(fg, bg, 0.65)),
    '--accent': css(accent),
    '--accent-hover': css(accentHover),
    '--text-on-accent': css(onAccent),
    '--focus-ring': css(accent, 0.18),
    '--ring': `0 0 0 3px ${css(accent, 0.25)}`,
  };
}

let themes: ExtContributions['themes'] = [];
let loaded: Promise<void> | null = null;
let appliedKey = '';

async function load(): Promise<void> {
  const c = await window.void.extContributions().catch(() => ({ themes: [], snippets: [] }) as ExtContributions);
  themes = c.themes;
}

function clear(): void {
  const root = document.documentElement;
  for (const v of VARS) root.style.removeProperty(v);
  delete root.dataset.extTheme;
  appliedKey = '';
}

export function applyWorkbenchTheme(s: AppSettings): void {
  const key = s.themeWorkbench === false ? '' : s.editorTheme;
  const t = key ? themes.find((x) => themeKey(x.extId, x.label) === key) : undefined;
  const vars = t ? workbenchVars(t.json, t.base) : null;
  if (!vars) { if (appliedKey) clear(); return; }
  const root = document.documentElement;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  root.dataset.extTheme = t!.base === 'vs' ? 'light' : 'dark';
  appliedKey = key;
}

export function initWorkbenchTheme(): void {
  loaded = load().then(() => applyWorkbenchTheme(state.settings));
  bus.on('settings-changed', (s) => void loaded?.then(() => applyWorkbenchTheme(s)));
  bus.on('extensions-changed', () => { loaded = load().then(() => applyWorkbenchTheme(state.settings)); });
}
