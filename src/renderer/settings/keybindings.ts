// Editable keybindings section for the settings tab: per-action rebind via
// click-to-capture, reset to default, and VSCode keybindings.json import.

import { KEYBIND_DEFS } from '../../shared/types';
import type { KeybindAction } from '../../shared/types';
import { el, escapeHtml, state } from '../state';
import { apply, settingBlock } from './controls';
import { chordFor, chordFromEvent, normalizeChord, formatChord, importVscodeKeybindings } from '../keybindings';

async function setBinding(action: KeybindAction, chord: string): Promise<void> {
  const next = { ...(state.settings.keybindings ?? {}), [action]: chord };
  await apply({ keybindings: next });
}

async function resetBinding(action: KeybindAction): Promise<void> {
  const next = { ...(state.settings.keybindings ?? {}) };
  delete next[action];
  await apply({ keybindings: next });
}

/** Which action (other than `self`) currently uses `chord`, if any. */
function conflictFor(chord: string, self: KeybindAction): string | null {
  const norm = normalizeChord(chord);
  for (const d of KEYBIND_DEFS) {
    if (d.action === self) continue;
    if (normalizeChord(chordFor(d.action)) === norm) return d.label;
  }
  return null;
}

export function buildKeybindingsSection(): HTMLElement[] {
  const table = el('div', 'kb-table');

  function render(): void {
    table.textContent = '';
    for (const def of KEYBIND_DEFS) {
      const row = el('div', 'kb-row');
      row.appendChild(el('span', 'kb-action', escapeHtml(def.label)));

      const chord = chordFor(def.action);
      const keyBtn = el('button', 'kb-keys-btn', formatChord(chord) || 'Set');
      keyBtn.title = chord;

      const resetBtn = el('button', 'small-btn kb-reset', 'Reset');
      const isCustom = Boolean(state.settings.keybindings?.[def.action]);
      resetBtn.style.visibility = isCustom ? 'visible' : 'hidden';

      keyBtn.addEventListener('click', () => startCapture(def.action, def.label, keyBtn, render));
      resetBtn.addEventListener('click', () => void resetBinding(def.action).then(render));

      const right = el('div', 'kb-row-right');
      right.append(keyBtn, resetBtn);
      row.appendChild(right);
      table.appendChild(row);
    }
  }
  render();

  // VSCode import control.
  const importWrap = el('div', 'kb-import');
  const importBtn = el('button', '', 'Import from VSCode…');
  const importStatus = el('span', 'kb-import-status');
  const fileInput = el('input') as HTMLInputElement;
  fileInput.type = 'file';
  fileInput.accept = '.json,application/json';
  fileInput.style.display = 'none';
  importBtn.addEventListener('click', () => fileInput.click());
  fileInput.addEventListener('change', () => {
    const file = fileInput.files?.[0];
    fileInput.value = '';
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      try {
        const res = importVscodeKeybindings(String(reader.result));
        if (res.matched === 0) {
          importStatus.className = 'kb-import-status err';
          importStatus.textContent = 'No matching shortcuts found in that file.';
          return;
        }
        void apply({ keybindings: { ...(state.settings.keybindings ?? {}), ...res.bindings } }).then(() => {
          render();
          importStatus.className = 'kb-import-status ok';
          importStatus.textContent = `Imported ${res.matched} shortcut${res.matched === 1 ? '' : 's'} from VSCode.`;
        });
      } catch (err) {
        importStatus.className = 'kb-import-status err';
        importStatus.textContent = err instanceof Error ? err.message : 'Import failed.';
      }
    };
    reader.readAsText(file);
  });
  importWrap.append(importBtn, importStatus, fileInput);

  // Fixed, context-specific shortcuts that aren't globally rebindable.
  const fixed = el('div', 'kb-table');
  for (const [keys, label] of [['Enter', 'Send AI message'], [formatChord('Cmd+Y'), 'Accept diff'], [formatChord('Cmd+N'), 'Reject diff']]) {
    const r = el('div', 'kb-row');
    r.appendChild(el('span', 'kb-action', escapeHtml(label)));
    const right = el('div', 'kb-row-right');
    right.appendChild(el('span', 'kb-keys', escapeHtml(keys)));
    r.appendChild(right);
    fixed.appendChild(r);
  }

  return [
    settingBlock('Shortcuts', 'Click a shortcut to rebind it. Press the new keys, or Esc to cancel.', table),
    settingBlock('Import', 'Bring your shortcuts over from VSCode (Code → Preferences → Keyboard Shortcuts → open keybindings.json).', importWrap),
    settingBlock('Other shortcuts', 'Context-specific shortcuts (not rebindable yet).', fixed),
  ];
}

/** Capture the next chord the user presses and assign it to `action`. */
function startCapture(action: KeybindAction, label: string, btn: HTMLElement, rerender: () => void): void {
  btn.classList.add('capturing');
  btn.textContent = 'Press keys…';

  const finish = (): void => {
    window.removeEventListener('keydown', onKey, true);
    btn.classList.remove('capturing');
    rerender();
  };

  const onKey = (e: KeyboardEvent): void => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      finish();
      return;
    }
    // Wait until a non-modifier key is pressed.
    if (['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) return;
    const chord = normalizeChord(chordFromEvent(e));
    if (!chord) return;
    const conflict = conflictFor(chord, action);
    window.removeEventListener('keydown', onKey, true);
    btn.classList.remove('capturing');
    if (conflict) {
      btn.classList.add('kb-conflict');
      btn.textContent = `Used by ${conflict}`;
      window.setTimeout(() => {
        btn.classList.remove('kb-conflict');
        rerender();
      }, 1600);
      return;
    }
    void setBinding(action, chord).then(rerender);
  };
  window.addEventListener('keydown', onKey, true);
}
