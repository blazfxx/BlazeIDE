// Editor area: Monaco lazy-loaded on first file open, multi-tab with model
// swapping, 500ms-debounced autosave, AI diff review, custom-DOM tabs.

import { applyExtensions, editorThemeName } from '../extensions/apply';
import { loadMonaco } from '../monaco-loader-client';
import { registerAutocomplete } from './autocomplete';
import type { Monaco } from '../monaco-loader-client';
import type * as MonacoTypes from 'monaco-editor';
import type { AppSettings } from '../../shared/types';
import { bus, el, state } from '../state';
import { runEditCommand } from '../layout/winTitlebar';
import { TabStrip } from './tabs';
import { languageForPath } from './languages';
import { createDiffSession } from './diff';
import { defineVoidTheme, editorOptions, monacoThemeFor } from './theme';
import { createSurfaces } from './surfaces';
import { showContextMenu } from '../sidebar/contextMenu';

export interface EditorArea {
  openFile(path: string): Promise<void>;
  openCustomTab(id: string, title: string, render: (host: HTMLElement) => void): void;
  saveActive(): Promise<void>;
  closeActive(): void;
  getActive(): { path: string; content: string; selection: string } | null;
  insertAtCursor(text: string): void;
  showDiff(path: string, original: string, modified: string): Promise<boolean>;
  /** If `path` is open, replace its editor content (used after the agent writes
   *  a file to disk so the open view isn't stale). No-op if not open. */
  syncOpenFile(path: string, content: string): void;
  /** Live-stream generated content into a file's editor view as the model
   *  writes it (Cursor-style). Opens/creates the tab on first call, replaces
   *  the model value on each call, and keeps the view scrolled to the end.
   *  Does not write to disk — the agent loop persists the final content. */
  streamInto(path: string, content: string): Promise<void>;
  applySettings(s: AppSettings): void;
  /** Re-read installed extensions (themes, snippets) into Monaco. */
  reloadExtensions(): void;
  hasOpenTabs(): boolean;
  /** Load Monaco in the background so the first file open is instant. */
  prewarm(): void;
}

interface FileDoc {
  model: MonacoTypes.editor.ITextModel;
  saveTimer: number | null;
  viewState: MonacoTypes.editor.ICodeEditorViewState | null;
}
const AUTOSAVE_MS = 500;

export function initEditorArea(tabbarEl: HTMLElement, hostEl: HTMLElement): EditorArea {
  let monaco: Monaco | null = null;
  let editor: MonacoTypes.editor.IStandaloneCodeEditor | null = null;
  const docs = new Map<string, FileDoc>();
  const customEls = new Map<string, HTMLElement>();
  // Paths currently being live-streamed by the agent — suppress dirty/autosave
  // while their content is mid-flight (the agent persists the final version).
  const streamingDocs = new Set<string>();
  // Serializes streamInto calls so concurrent (un-awaited) deltas can't race on
  // model creation. Each call chains after the previous one resolves.
  let streamChain: Promise<void> = Promise.resolve();

  const surfaces = createSurfaces(hostEl);
  const { editorEl, customHost, show: showSurface } = surfaces;

  const tabs = new TabStrip(tabbarEl, {
    onActivate: (id) => activate(id),
    onClose: (id) => close(id),
    onContextMenu: (id, e) => {
      const tab = tabs.get(id);
      if (!tab || tab.kind !== 'file') return;
      showContextMenu(e.clientX, e.clientY, [
        {
          label: 'Open in New Window',
          action: () => {
            void saveDoc(id).then(() => window.void.winOpenFile(id, state.projectRoot));
            close(id);
          },
        },
        { label: 'Close Tab', action: () => close(id) },
      ]);
    },
  });

  async function ensureMonaco(): Promise<Monaco> {
    if (monaco) return monaco;
    const m = await loadMonaco();
    if (monaco) return monaco; // raced
    monaco = m;
    defineVoidTheme(m);
    registerAutocomplete(m);
    void applyExtensions(m, state.settings);
    // Keep the heavy TypeScript worker (~11MB) off the critical path of just
    // *showing* a file. Coloring is done by the main-thread Monarch tokenizer
    // and needs no worker; only diagnostics/IntelliSense do. So we open with
    // them OFF (instant first paint), then re-enable diagnostics during idle.
    // (Technique from VS Code: color now on the main thread, hydrate semantics
    // a beat later.)
    try {
      const ts = m.languages.typescript;
      const off = { noSemanticValidation: true, noSyntaxValidation: true, noSuggestionDiagnostics: true, onlyVisible: true };
      ts.typescriptDefaults.setDiagnosticsOptions(off);
      ts.javascriptDefaults.setDiagnosticsOptions(off);
      // Don't dump every open model into the worker the instant it boots.
      ts.typescriptDefaults.setEagerModelSync(false);
      ts.javascriptDefaults.setEagerModelSync(false);
      // After first paint settles, turn syntax + suggestion diagnostics back on
      // (visible files only) so error/warning badges still work — semantic
      // validation stays off (it's the expensive, project-wide pass we don't
      // need in a lightweight editor).
      const reenable = (): void => {
        const on = { noSemanticValidation: true, noSyntaxValidation: false, noSuggestionDiagnostics: false, onlyVisible: true };
        ts.typescriptDefaults.setDiagnosticsOptions(on);
        ts.javascriptDefaults.setDiagnosticsOptions(on);
      };
      const ric = (window as unknown as { requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => void }).requestIdleCallback;
      if (ric) ric(reenable, { timeout: 4000 });
      else window.setTimeout(reenable, 1500);
    } catch {
      // Older/typeless Monaco build — safe to skip.
    }
    editor = m.editor.create(editorEl, {
      ...editorOptions(state.settings),
      theme: monacoThemeFor(state.settings.theme),
      model: null,
    });
    editor.onDidChangeCursorPosition((e) => {
      bus.emit('cursor-changed', { line: e.position.lineNumber, col: e.position.column });
    });
    editor.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => {
      void saveActive();
    });
    m.editor.onDidChangeMarkers(() => {
      let errors = 0;
      let warnings = 0;
      // Per-file counts for the tab badges, plus the global total for the bar.
      const perFile = new Map<string, { errors: number; warnings: number }>();
      for (const marker of m.editor.getModelMarkers({})) {
        const isErr = marker.severity === m.MarkerSeverity.Error;
        const isWarn = marker.severity === m.MarkerSeverity.Warning;
        if (isErr) errors++;
        else if (isWarn) warnings++;
        if (!isErr && !isWarn) continue;
        const path = marker.resource.path;
        const c = perFile.get(path) ?? { errors: 0, warnings: 0 };
        if (isErr) c.errors++;
        else c.warnings++;
        perFile.set(path, c);
      }
      // Update every open file tab (clearing those with no markers).
      for (const t of tabs.list()) {
        if (t.kind !== 'file') continue;
        const c = perFile.get(m.Uri.file(t.id).path);
        tabs.setMarkers(t.id, c?.errors ?? 0, c?.warnings ?? 0);
      }
      bus.emit('markers-changed', { errors, warnings });
    });
    return m;
  }

  function activate(id: string, preserveFocus = false): void {
    const tab = tabs.get(id);
    if (!tab) return;
    const prev = tabs.getActive();
    if (prev && prev.kind === 'file' && editor) {
      const doc = docs.get(prev.id);
      if (doc) doc.viewState = editor.saveViewState();
    }
    tabs.activate(id);
    if (tab.kind === 'custom') {
      customHost.textContent = '';
      const customEl = customEls.get(id);
      if (customEl) customHost.appendChild(customEl);
      showSurface('custom');
      bus.emit('active-file-changed', { path: null });
      return;
    }
    const doc = docs.get(id);
    if (doc && editor) {
      editor.setModel(doc.model);
      if (doc.viewState) editor.restoreViewState(doc.viewState);
      showSurface('editor');
      // The editor is created while its container is display:none and measures
      // 0x0; automaticLayout alone does not always recover from that.
      editor.layout();
      if (!preserveFocus) editor.focus();
      bus.emit('active-file-changed', { path: id });
      bus.emit('editor-lang-changed', { lang: languageForPath(id) });
    }
  }

  async function saveDoc(path: string): Promise<void> {
    const doc = docs.get(path);
    if (!doc) return;
    if (doc.saveTimer !== null) {
      window.clearTimeout(doc.saveTimer);
      doc.saveTimer = null;
    }
    try {
      await window.void.fsWriteFile(path, doc.model.getValue());
      tabs.setDirty(path, false);
    } catch {
      // Keep the dirty dot; autosave will retry on next change.
    }
  }

  function scheduleAutosave(path: string): void {
    if (!state.settings.editorAutosave) return; // manual-save mode (Cmd+S)
    const doc = docs.get(path);
    if (!doc) return;
    if (doc.saveTimer !== null) window.clearTimeout(doc.saveTimer);
    doc.saveTimer = window.setTimeout(() => {
      void saveDoc(path);
    }, AUTOSAVE_MS);
  }

  async function openFile(path: string, preserveFocus = false): Promise<void> {
    // Images/videos can't go in a Monaco text model — show them in a media
    // viewer custom tab (served via the blazemedia: protocol).
    if (isMediaPath(path)) {
      openMediaTab(path);
      return;
    }
    if (docs.has(path)) {
      await ensureMonaco();
      if (!tabs.get(path)) {
        tabs.add({ id: path, title: basename(path), kind: 'file', dirty: false });
      }
      activate(path, preserveFocus);
      return;
    }
    // Read the file and load Monaco concurrently — on first open the (cold)
    // Monaco load and the disk read overlap; afterwards the read dominates.
    const [m, content] = await Promise.all([
      ensureMonaco(),
      window.void.fsReadFile(path).catch(() => null),
    ]);
    if (content === null) return; // unreadable (binary/permission) — ignore the click
    if (docs.has(path)) {
      activate(path, preserveFocus);
      return; // double-click raced us
    }
    const model = m.editor.createModel(content, languageForPath(path), m.Uri.file(path));
    const doc: FileDoc = { model, saveTimer: null, viewState: null };
    docs.set(path, doc);
    model.onDidChangeContent(() => {
      if (streamingDocs.has(path)) return; // live agent stream — not a user edit
      tabs.setDirty(path, true);
      scheduleAutosave(path);
    });
    tabs.add({ id: path, title: basename(path), kind: 'file', dirty: false });
    activate(path, preserveFocus);
  }

  function close(id: string): void {
    const tab = tabs.get(id);
    if (!tab) return;
    if (tab.kind === 'file') {
      const doc = docs.get(id);
      if (doc) {
        if (doc.saveTimer !== null) {
          window.clearTimeout(doc.saveTimer);
          void saveDoc(id); // flush pending edits before closing
        }
        doc.model.dispose();
        docs.delete(id);
      }
    } else {
      customEls.delete(id);
    }
    const nextId = tabs.remove(id);
    if (nextId) activate(nextId);
    else {
      editor?.setModel(null);
      showSurface('empty');
      bus.emit('active-file-changed', { path: null });
    }
  }

  function basename(p: string): string {
    return p.split('/').pop() ?? p;
  }

  const IMAGE_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'svg', 'ico', 'bmp']);
  const VIDEO_EXT = new Set(['mp4', 'webm', 'mov', 'm4v']);
  function extOf(p: string): string {
    return p.split('.').pop()?.toLowerCase() ?? '';
  }
  function isMediaPath(p: string): boolean {
    const e = extOf(p);
    return IMAGE_EXT.has(e) || VIDEO_EXT.has(e);
  }
  function mediaUrl(abs: string): string {
    // Cache-bust with mtime-ish counter so a regenerated asset re-renders.
    return `${window.void.mediaUrl(abs)}?t=${mediaBust++}`;
  }
  let mediaBust = 0;

  /** Open (or refresh) an image/video in a custom viewer tab. */
  function openMediaTab(path: string): void {
    const e = extOf(path);
    const isVideo = VIDEO_EXT.has(e);
    area.openCustomTab(`media:${path}`, basename(path), (host) => {
      host.style.cssText = 'height:100%;display:flex;align-items:center;justify-content:center;background:var(--bg-base);overflow:auto;padding:16px;';
      const url = mediaUrl(path);
      const elMedia = isVideo
        ? Object.assign(document.createElement('video'), { src: url, controls: true, loop: true, muted: true, autoplay: true })
        : Object.assign(document.createElement('img'), { src: url, alt: basename(path) });
      elMedia.style.cssText = 'max-width:100%;max-height:100%;object-fit:contain;border-radius:8px;box-shadow:0 4px 24px var(--shadow);';
      host.appendChild(elMedia);
    });
  }

  async function saveActive(): Promise<void> {
    const tab = tabs.getActive();
    if (tab && tab.kind === 'file') await saveDoc(tab.id);
  }

  const area: EditorArea = {
    openFile,
    openCustomTab(id, title, render) {
      // Re-render on every open so tabs like Settings always show current
      // values; the container element itself is stable per tab id.
      let customEl = customEls.get(id);
      if (!customEl) {
        customEl = el('div', 'custom-tab-content');
        customEls.set(id, customEl);
      }
      customEl.textContent = '';
      render(customEl);
      tabs.add({ id, title, kind: 'custom', dirty: false });
      activate(id);
    },
    saveActive,
    closeActive() {
      const tab = tabs.getActive();
      if (tab) close(tab.id);
    },
    getActive() {
      const tab = tabs.getActive();
      if (!tab || tab.kind !== 'file' || !editor) return null;
      const doc = docs.get(tab.id);
      if (!doc) return null;
      const sel = editor.getSelection();
      const selection = sel && !sel.isEmpty() ? doc.model.getValueInRange(sel) : '';
      return { path: tab.id, content: doc.model.getValue(), selection };
    },
    insertAtCursor(text) {
      if (!editor) return;
      const pos = editor.getPosition();
      if (!pos) return;
      editor.executeEdits('ai-insert', [
        {
          range: { startLineNumber: pos.lineNumber, startColumn: pos.column, endLineNumber: pos.lineNumber, endColumn: pos.column },
          text,
        },
      ]);
      editor.focus();
    },
    async showDiff(path, original, modified) {
      await ensureMonaco();
      const session = await createDiffSession(original, modified, languageForPath(path));
      const tabId = `diff:${path}`;
      area.openCustomTab(tabId, `Review: ${basename(path)}`, (host) => {
        host.style.height = '100%';
        host.appendChild(session.container);
      });
      const accepted = await session.result;
      if (accepted) {
        const finalContent = session.getModified();
        await window.void.fsWriteFile(path, finalContent);
        const doc = docs.get(path);
        if (doc && doc.model.getValue() !== finalContent) doc.model.setValue(finalContent);
        tabs.setDirty(path, false);
      }
      session.dispose();
      close(tabId);
      return accepted;
    },
    syncOpenFile(path, content) {
      streamingDocs.delete(path); // streaming (if any) is done
      const doc = docs.get(path);
      if (doc && doc.model.getValue() !== content) doc.model.setValue(content);
      tabs.setDirty(path, false);
    },
    async streamInto(path, content) {
      // Chain so rapid un-awaited calls serialize (no model-creation race).
      const run = streamChain.then(async () => {
        const m = await ensureMonaco();
        let doc = docs.get(path);
        if (!doc) {
          // Create a tab/model for the file even if it doesn't exist on disk yet
          // (new file being written). Mirror openFile's model wiring.
          const model = m.editor.createModel('', languageForPath(path), m.Uri.file(path));
          doc = { model, saveTimer: null, viewState: null };
          docs.set(path, doc);
          model.onDidChangeContent(() => {
            if (streamingDocs.has(path)) return;
            tabs.setDirty(path, true);
            scheduleAutosave(path);
          });
          tabs.add({ id: path, title: basename(path), kind: 'file', dirty: false });
        }
        streamingDocs.add(path);
        if (tabs.getActive()?.id !== path) activate(path);
        if (doc.model.getValue() !== content) {
          doc.model.setValue(content);
          // Follow the tail so the user watches new code appear.
          if (editor && tabs.getActive()?.id === path) {
            editor.revealLine(doc.model.getLineCount());
          }
        }
      }).catch(() => undefined);
      streamChain = run;
      return run;
    },
    reloadExtensions() {
      if (monaco) void applyExtensions(monaco, state.settings);
    },
    applySettings(s) {
      editor?.updateOptions(editorOptions(s));
      monaco?.editor.setTheme(editorThemeName(s));
    },
    hasOpenTabs() {
      return !tabs.isEmpty();
    },
    prewarm() {
      // Idle-time load: don't compete with initial paint or project restore.
      // loadMonaco caches its promise, so a failure here just defers the
      // error to the first real openFile().
      window.setTimeout(() => {
        void ensureMonaco().catch((err: unknown) => {
          console.error('monaco prewarm failed', err);
        });
      }, 150);
    },
  };

  bus.on('open-file', ({ path, preserveFocus }) => void openFile(path, preserveFocus));
  bus.on('ai-insert-at-cursor', ({ text }) => area.insertAtCursor(text));
  bus.on('ai-apply-edit', ({ path, original, modified }) => void area.showDiff(path, original, modified));
  bus.on('settings-changed', (s) => area.applySettings(s));
  bus.on('edit-command', (cmd) => {
    runEditCommand(cmd, (id) => {
      if (!editor || !editor.hasTextFocus()) return false;
      editor.focus();
      editor.trigger('menu', id, null);
      return true;
    });
  });
  bus.on('extensions-changed', () => area.reloadExtensions());

  showSurface('empty');
  return area;
}
