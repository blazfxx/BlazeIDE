// Keybinding registry: resolves chords from settings (falling back to
// defaults), matches keyboard events to actions, and imports VSCode keybinds.
// Chords are stored in a normalized form like "Cmd+Shift+A" / "Ctrl+`".
// "Cmd" means the platform's primary modifier: Command on macOS, Ctrl on
// Windows/Linux. Off macOS "Ctrl" folds into it too, so one stored chord works
// everywhere.

import type { KeybindAction } from '../shared/types';
import { DEFAULT_KEYBINDINGS, KEYBIND_DEFS } from '../shared/types';
import { state } from './state';

const MOD_ORDER = ['Cmd', 'Ctrl', 'Alt', 'Shift'];
const IS_MAC = typeof navigator !== 'undefined' && /Mac OS X|Macintosh/.test(navigator.userAgent);

/** True when the platform's primary modifier (Cmd on mac, Ctrl elsewhere) is held. */
export function primaryMod(e: KeyboardEvent | MouseEvent): boolean {
  return IS_MAC ? e.metaKey : e.ctrlKey;
}

/** Map a stored chord to this platform (off mac, Ctrl and Cmd are the same key). */
function platformChord(chord: string): string {
  if (IS_MAC) return chord;
  const toks = chord.split('+').map((t) => (t.toLowerCase() === 'ctrl' ? 'Cmd' : t));
  return [...new Set(toks)].join('+');
}

/** Effective chord for an action: user override or built-in default. */
export function chordFor(action: KeybindAction): string {
  return state.settings.keybindings?.[action] || DEFAULT_KEYBINDINGS[action] || '';
}

/** Normalize a raw key name to our canonical token (single chars upper-cased). */
function normalizeKey(key: string): string {
  if (key === ' ') return 'Space';
  if (key.length === 1) return key.toUpperCase();
  // Arrow/Function/etc. keep their DOM name (e.g. "ArrowLeft", "Enter").
  return key;
}

/** Build the canonical chord string from a keyboard event. */
export function chordFromEvent(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (IS_MAC) {
    if (e.metaKey) parts.push('Cmd');
    if (e.ctrlKey) parts.push('Ctrl');
  } else if (e.ctrlKey) {
    parts.push('Cmd'); // the Windows key (metaKey) is never part of a chord
  }
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  const k = normalizeKey(e.key);
  // Don't treat a lone modifier press as a chord.
  if (!['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) parts.push(k);
  return parts.join('+');
}

/** Reorder/clean a stored chord so comparisons are stable. */
export function normalizeChord(chord: string): string {
  const tokens = chord.split('+').map((t) => t.trim()).filter(Boolean);
  const mods = MOD_ORDER.filter((m) => tokens.some((t) => t.toLowerCase() === m.toLowerCase()));
  const keys = tokens.filter((t) => !MOD_ORDER.some((m) => m.toLowerCase() === t.toLowerCase()));
  const key = keys.length ? normalizeKey(keys[keys.length - 1]) : '';
  return [...mods, key].filter(Boolean).join('+');
}

/** Pretty display: symbols on mac (⌘ ⌥ ⌃ ⇧), words elsewhere (Ctrl+Shift+A). */
export function formatChord(chord: string): string {
  if (!IS_MAC) {
    return platformChord(chord)
      .split('+')
      .map((t) => (t === 'Cmd' ? 'Ctrl' : t))
      .join('+');
  }
  return chord
    .split('+')
    .map((t) => {
      switch (t) {
        case 'Cmd': return '⌘';
        case 'Ctrl': return '⌃';
        case 'Alt': return '⌥';
        case 'Shift': return '⇧';
        default: return t;
      }
    })
    .join('');
}

/** The action whose chord matches this event, or null. */
export function matchAction(e: KeyboardEvent): KeybindAction | null {
  const chord = normalizeChord(chordFromEvent(e));
  if (!chord) return null;
  for (const def of KEYBIND_DEFS) {
    if (normalizeChord(platformChord(chordFor(def.action))) === chord) return def.action;
  }
  return null;
}

// ---- VSCode import -------------------------------------------------------

// Maps VSCode command ids to our actions. Multiple VSCode commands can map to
// the same Blaze IDE action; the first matching binding wins.
const VSCODE_COMMAND_MAP: Record<string, KeybindAction> = {
  'workbench.action.toggleSidebarVisibility': 'toggle-sidebar',
  'workbench.action.toggleAuxiliaryBar': 'toggle-ai-panel',
  'workbench.action.terminal.toggleTerminal': 'toggle-terminal',
  'workbench.action.showCommands': 'command-palette',
  'workbench.action.quickOpen': 'file-palette',
  'workbench.action.files.save': 'save',
  'workbench.action.closeActiveEditor': 'close-tab',
  'workbench.action.openSettings': 'settings',
  'workbench.action.openSettings2': 'settings',
  'workbench.action.files.openFolder': 'open-folder',
  'workbench.action.files.openFileFolder': 'open-folder',
};

/** Convert a VSCode "key" string (e.g. "cmd+shift+a", "ctrl+oem_3") to our
 *  canonical chord, or null if it can't be represented. */
function vscodeKeyToChord(key: string): string | null {
  const tokens = key.split('+').map((t) => t.trim().toLowerCase()).filter(Boolean);
  if (tokens.length === 0) return null;
  const out: string[] = [];
  let main = '';
  for (const t of tokens) {
    if (t === 'cmd' || t === 'meta' || t === 'win' || t === 'super') out.push('Cmd');
    else if (t === 'ctrl' || t === 'control') out.push('Ctrl');
    else if (t === 'alt' || t === 'option') out.push('Alt');
    else if (t === 'shift') out.push('Shift');
    else main = vscodeMainKey(t);
  }
  if (!main) return null;
  return normalizeChord([...out, main].join('+'));
}

function vscodeMainKey(t: string): string {
  const map: Record<string, string> = {
    oem_3: '`', backquote: '`', oem_comma: ',', oem_period: '.',
    oem_minus: '-', oem_plus: '=', space: 'Space', enter: 'Enter',
    escape: 'Escape', tab: 'Tab', backspace: 'Backspace', delete: 'Delete',
    left: 'ArrowLeft', right: 'ArrowRight', up: 'ArrowUp', down: 'ArrowDown',
  };
  if (map[t]) return map[t];
  if (t.length === 1) return t.toUpperCase();
  return t;
}

export interface ImportResult {
  bindings: Record<string, string>;
  matched: number;
  total: number;
}

/** Parse a VSCode keybindings.json (JSONC) and map recognized commands to Blaze IDE
 *  actions. Tolerates comments and trailing commas. */
export function importVscodeKeybindings(text: string): ImportResult {
  const cleaned = stripJsonc(text);
  let parsed: unknown;
  try {
    parsed = JSON.parse(cleaned);
  } catch {
    throw new Error('That file is not valid JSON. Use your VSCode keybindings.json.');
  }
  if (!Array.isArray(parsed)) {
    throw new Error('Unexpected format — expected a VSCode keybindings array.');
  }
  const bindings: Record<string, string> = {};
  let total = 0;
  for (const entry of parsed) {
    if (!entry || typeof entry !== 'object') continue;
    const o = entry as Record<string, unknown>;
    const cmd = typeof o.command === 'string' ? o.command : '';
    const key = typeof o.key === 'string' ? o.key : '';
    if (!cmd || !key || cmd.startsWith('-')) continue; // '-' prefix = removal
    const action = VSCODE_COMMAND_MAP[cmd];
    if (!action) continue;
    total++;
    const chord = vscodeKeyToChord(key);
    if (chord && !(action in bindings)) bindings[action] = chord;
  }
  return { bindings, matched: Object.keys(bindings).length, total };
}

/** Strip // and /* *​/ comments and trailing commas from JSONC. */
function stripJsonc(text: string): string {
  const noComments = text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1');
  return noComments.replace(/,(\s*[}\]])/g, '$1');
}
