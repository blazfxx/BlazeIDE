// Diff review tab: side-by-side Monaco diff with Accept (Cmd+Y) / Reject
// (Cmd+N). Resolves true on accept; the editor module writes the file.

import { formatChord, primaryMod } from '../keybindings';
import { loadMonaco } from '../monaco-loader-client';
import type { Monaco } from '../monaco-loader-client';
import { el } from '../state';

export interface DiffSession {
  container: HTMLElement;
  result: Promise<boolean>;
  /** Current modified-side content (the user may tweak the AI edit in place). */
  getModified(): string;
  dispose(): void;
}

export async function createDiffSession(
  original: string,
  modified: string,
  language: string
): Promise<DiffSession> {
  const monaco: Monaco = await loadMonaco();

  const container = el('div', 'diff-session');
  container.style.cssText = 'display:flex;flex-direction:column;height:100%;';

  const header = el('div', 'diff-header');
  header.style.cssText =
    'display:flex;align-items:center;gap:8px;padding:6px 12px;border-bottom:1px solid var(--border);';
  const hint = el('span', 'diff-hint', 'Review AI edit');
  hint.style.cssText = 'color:var(--text-secondary);font-size:12px;margin-right:auto;';
  const acceptBtn = el('button', 'primary', `Accept ${formatChord('Cmd+Y')}`);
  const rejectBtn = el('button', '', `Reject ${formatChord('Cmd+N')}`);
  header.append(hint, acceptBtn, rejectBtn);

  const editorHost = el('div', 'diff-editor-host');
  editorHost.style.cssText = 'flex:1;min-height:0;';
  container.append(header, editorHost);

  const originalModel = monaco.editor.createModel(original, language);
  const modifiedModel = monaco.editor.createModel(modified, language);
  const diffEditor = monaco.editor.createDiffEditor(editorHost, {
    renderSideBySide: true,
    readOnly: false,
    originalEditable: false,
    automaticLayout: true,
    minimap: { enabled: false },
    theme: 'void-dark',
    fontSize: 13,
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', 'Menlo', 'Consolas', monospace",
  });
  diffEditor.setModel({ original: originalModel, modified: modifiedModel });

  let settle: (v: boolean) => void;
  const result = new Promise<boolean>((resolve) => {
    settle = resolve;
  });

  const onKey = (e: KeyboardEvent): void => {
    if (!primaryMod(e)) return;
    if (e.key.toLowerCase() === 'y') {
      e.preventDefault();
      e.stopPropagation();
      settle(true);
    } else if (e.key.toLowerCase() === 'n') {
      e.preventDefault();
      e.stopPropagation();
      settle(false);
    }
  };
  window.addEventListener('keydown', onKey, true);

  acceptBtn.addEventListener('click', () => settle(true));
  rejectBtn.addEventListener('click', () => settle(false));

  const dispose = (): void => {
    window.removeEventListener('keydown', onKey, true);
    diffEditor.dispose();
    originalModel.dispose();
    modifiedModel.dispose();
  };

  return {
    container,
    result,
    getModified: () => modifiedModel.getValue(),
    dispose,
  };
}
