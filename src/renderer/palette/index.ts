// Command palette: fuzzy file search (Cmd+P) and command list (Cmd+K).
// '>' prefix switches files mode to commands; removing it switches back.

import { openSkillsLibrary } from '../skills/library';
import type { FileNode } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { iconSearch } from '../icons/index';
import type { EditorArea } from '../editor/index';

interface Entry {
  label: string;
  detail: string;
  run(): void;
}

let editorRef: EditorArea | null = null;
let openEl: HTMLElement | null = null;

/** Subsequence fuzzy score; -1 if query isn't a subsequence of target. */
function fuzzyScore(query: string, target: string): number {
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  let score = 0;
  let ti = 0;
  let prevMatch = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const idx = t.indexOf(q[qi], ti);
    if (idx < 0) return -1;
    if (idx === prevMatch + 1) score += 2;
    else if (idx === 0 || t[idx - 1] === '/' || t[idx - 1] === '.' || t[idx - 1] === '-' || t[idx - 1] === '_') score += 3;
    else score += 1;
    prevMatch = idx;
    ti = idx + 1;
  }
  return score;
}

function flattenTree(node: FileNode, out: string[]): void {
  for (const child of node.children ?? []) {
    if (child.isDirectory) flattenTree(child, out);
    else out.push(child.path);
  }
}

function commandEntries(): Entry[] {
  const e = (label: string, run: () => void): Entry => ({ label, detail: '', run });
  return [
    e('Toggle Sidebar', () => bus.emit('toggle-sidebar', undefined)),
    e('Toggle AI Panel', () => bus.emit('toggle-ai-panel', undefined)),
    e('Toggle Terminal', () => bus.emit('toggle-terminal', undefined)),
    e('New Terminal', () => bus.emit('new-terminal', undefined)),
    e('New Window', () => void window.void.winNew()),
    e('Open Folder', () => {
      void window.void.fsOpenFolder().then((root) => {
        if (!root) return;
        const name = root.split('/').pop() ?? root;
        state.projectRoot = root;
        state.projectName = name;
        void window.void.fsWatch(root);
        bus.emit('project-opened', { root, name });
      });
    }),
    e('Open Settings', () => bus.emit('open-settings', undefined)),
    e('Skills Library', () => openSkillsLibrary()),
    e('Skills: Import from Claude Code, Codex, OpenCode', () => openSkillsLibrary({ tab: 'import' })),
    e('Open Web Preview', () => bus.emit('open-preview', {})),
    e('Account', () => bus.emit('open-account', undefined)),
    e('Save', () => void editorRef?.saveActive()),
    e('Close Tab', () => editorRef?.closeActive()),
    e('AI: Ask about current file', () => bus.emit('focus-ai-input', undefined)),
    e('AI: New chat', () => bus.emit('focus-ai-input', undefined)),
  ];
}

export function openCommandPalette(mode: 'commands' | 'files'): void {
  closePalette();
  const overlay = document.getElementById('overlay-root');
  if (!overlay) return;

  const palette = el('div', 'palette');
  const inputRow = el('div', 'palette-input-row');
  inputRow.appendChild(el('span', 'icon palette-icon', iconSearch));
  const input = el('input', 'palette-input');
  input.placeholder = mode === 'files' ? 'search files — type > for commands' : 'run a command';
  input.spellcheck = false;
  if (mode === 'commands') input.value = '>';
  inputRow.appendChild(input);
  const listEl = el('div', 'palette-list');
  palette.append(inputRow, listEl);
  overlay.appendChild(palette);
  openEl = palette;

  let filePaths: string[] | null = null;
  let entries: Entry[] = [];
  let selected = 0;

  async function ensureFiles(): Promise<string[]> {
    if (filePaths) return filePaths;
    filePaths = [];
    if (state.projectRoot) {
      try {
        const tree = await window.void.fsReadTree(state.projectRoot);
        flattenTree(tree, filePaths);
      } catch {
        // no project / unreadable — empty list
      }
    }
    return filePaths;
  }

  function fileEntry(path: string): Entry {
    const rel = state.projectRoot ? path.slice(state.projectRoot.length + 1) : path;
    const name = rel.split('/').pop() ?? rel;
    const dir = rel.slice(0, rel.length - name.length).replace(/\/$/, '');
    return {
      label: name,
      detail: dir,
      run: () => bus.emit('open-file', { path }),
    };
  }

  async function refresh(): Promise<void> {
    const raw = input.value;
    const isCommand = raw.startsWith('>');
    const query = (isCommand ? raw.slice(1) : raw).trim();

    if (isCommand) {
      const all = commandEntries();
      entries = query ? all.map((c) => ({ c, s: fuzzyScore(query, c.label) })).filter((x) => x.s >= 0).sort((a, b) => b.s - a.s).map((x) => x.c) : all;
    } else {
      const files = await ensureFiles();
      if (query) {
        entries = files
          .map((p) => ({ p, s: fuzzyScore(query, p.split('/').slice(-3).join('/')) }))
          .filter((x) => x.s >= 0)
          .sort((a, b) => b.s - a.s)
          .slice(0, 50)
          .map((x) => fileEntry(x.p));
      } else {
        entries = files.slice(0, 50).map(fileEntry);
      }
    }
    selected = 0;
    render();
  }

  function render(): void {
    listEl.textContent = '';
    entries.forEach((entry, i) => {
      const item = el('div', `palette-item${i === selected ? ' selected' : ''}`);
      item.appendChild(el('span', 'palette-label', escapeHtml(entry.label)));
      if (entry.detail) item.appendChild(el('span', 'palette-detail', escapeHtml(entry.detail)));
      item.addEventListener('click', () => {
        closePalette();
        entry.run();
      });
      listEl.appendChild(item);
    });
    if (entries.length === 0) listEl.appendChild(el('div', 'palette-empty', 'No matches'));
    listEl.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
  }

  input.addEventListener('input', () => void refresh());
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selected = entries.length > 0 ? (selected + 1) % entries.length : 0;
      render();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selected = entries.length > 0 ? (selected - 1 + entries.length) % entries.length : 0;
      render();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const entry = entries[selected];
      if (entry) {
        closePalette();
        entry.run();
      }
    } else if (e.key === 'Escape') {
      closePalette();
    }
  });

  void refresh();
  window.setTimeout(() => input.focus(), 0);
  document.addEventListener('mousedown', onAway, true);
}

function onAway(e: MouseEvent): void {
  if (openEl && !openEl.contains(e.target as Node)) closePalette();
}

export function closePalette(): void {
  openEl?.remove();
  openEl = null;
  document.removeEventListener('mousedown', onAway, true);
}

export function initPalette(editor: EditorArea): void {
  editorRef = editor;
  bus.on('open-palette', ({ mode }) => openCommandPalette(mode));
}
