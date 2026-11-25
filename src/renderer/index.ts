// Renderer entry: auth gate, then workbench assembly and global wiring.

import './bridge'; // installs window.void before anything else runs
import { normModelId } from './ai-panel/chatStore';
import type { MenuAction } from '../shared/ipc';
import { bus, state } from './state';
import { initLayout } from './layout/index';
import { initStatusbar } from './layout/statusbar';
import { initEditorArea } from './editor/index';
import type { EditorArea } from './editor/index';
import { initSidebar } from './sidebar/index';
import { initTerminalPanel } from './terminal/index';
import { initAIPanel } from './ai-panel/index';
import { showLoginScreen } from './auth/index';
import { showOnboarding } from './onboarding/index';
import { applyAppearance } from './appearance';
import { applyAmbient } from './ambientAudio';
import { openSettingsTab } from './settings/index';
import { initPresence } from './presence';
import { initPalette, openCommandPalette } from './palette/index';
import { initPreview } from './preview/index';
import { matchAction } from './keybindings';
import { playSplash } from './splash';
import { showForceUpdate } from './forceUpdate';
import { maybeShowUpdateBanner, showUpdateReady } from './updateBanner';
import type { VersionInfo } from '../shared/types';
import './account/index'; // registers the open-account bus listener

async function openFolderFlow(): Promise<void> {
  const root = await window.void.fsOpenFolder();
  if (!root) return;
  await openProject(root);
}

async function openProject(root: string): Promise<void> {
  const name = root.split(/[\\/]/).filter(Boolean).pop() ?? root;
  state.projectRoot = root;
  state.projectName = name;
  await window.void.fsWatch(root);
  bus.emit('project-opened', { root, name });
}

function dispatch(action: MenuAction, editor: EditorArea): void {
  switch (action) {
    case 'open-folder':
      void openFolderFlow();
      break;
    case 'new-file':
      // The sidebar owns inline creation; just make sure it's visible.
      bus.emit('open-palette', { mode: 'commands' });
      break;
    case 'new-window':
      void window.void.winNew();
      break;
    case 'save':
      void editor.saveActive();
      break;
    case 'close-tab':
      if (editor.hasOpenTabs()) editor.closeActive();
      else window.close();
      break;
    case 'toggle-sidebar':
      bus.emit('toggle-sidebar', undefined);
      break;
    case 'toggle-ai-panel':
      bus.emit('toggle-ai-panel', undefined);
      break;
    case 'toggle-terminal':
      bus.emit('toggle-terminal', undefined);
      break;
    case 'command-palette':
      openCommandPalette('commands');
      break;
    case 'settings':
      openSettingsTab(editor);
      break;
    case 'account':
      bus.emit('open-account', undefined);
      break;
  }
}

function bindKeys(editor: EditorArea): void {
  // F2 renames the selected file from anywhere (except text fields). Enter
  // renames only when focus is in the file tree (Enter is too overloaded to
  // grab globally).
  window.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' && e.key !== 'F2') return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const t = document.activeElement as HTMLElement | null;
    const inText = !!t && (
      t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' ||
      t.isContentEditable || !!t.closest('.monaco-editor')
    );
    if (inText) return;
    if (e.key === 'Enter' && !(t && t.closest('.tree-host'))) return;
    e.preventDefault();
    bus.emit('rename-selected-file', undefined);
  });

  window.addEventListener('keydown', (e) => {
    const action = matchAction(e);
    if (!action) return;
    // Cmd+Backspace also means "delete to line start" in a text field — only
    // treat it as file-delete when focus isn't in an editor/input. Bail BEFORE
    // preventDefault so the editor/input keeps its native behavior.
    if (action === 'delete-file') {
      const t = document.activeElement;
      const inText = !!t && (
        t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' ||
        (t as HTMLElement).isContentEditable || !!t.closest('.monaco-editor')
      );
      if (inText) return;
      e.preventDefault();
      bus.emit('delete-selected-file', undefined);
      return;
    }
    e.preventDefault();
    switch (action) {
      case 'toggle-sidebar': bus.emit('toggle-sidebar', undefined); break;
      case 'toggle-ai-panel': bus.emit('toggle-ai-panel', undefined); break;
      case 'toggle-terminal': bus.emit('toggle-terminal', undefined); break;
      case 'command-palette': openCommandPalette('commands'); break;
      case 'file-palette': openCommandPalette('files'); break;
      case 'save': void editor.saveActive(); break;
      case 'close-tab': dispatch('close-tab', editor); break;
      case 'settings': openSettingsTab(editor); break;
      case 'account': bus.emit('open-account', undefined); break;
      case 'open-folder': void openFolderFlow(); break;
    }
  });
}

async function boot(): Promise<void> {
  const appEl = document.getElementById('app');
  if (!appEl) return;

  // Force-update gate: if this build is below the backend's minimum, take over
  // the screen and stop. (Offline / errors resolve to mustUpdate=false, so a
  // network blip never locks the user out.) A non-forced newer version surfaces
  // as a soft banner once the UI is up (see below).
  let versionInfo: VersionInfo | null = null;
  try {
    versionInfo = await window.void.checkVersion();
    if (versionInfo.mustUpdate) { showForceUpdate(versionInfo); return; }
  } catch { /* non-fatal — proceed */ }

  // Pop-out windows carry ?file= and ?root=; skip the splash for those so the
  // file appears immediately. Fresh windows get the flicker-reveal.
  const params = new URLSearchParams(window.location.search);
  const popoutFile = params.get('file');
  const popoutRoot = params.get('root');
  const splashDone = popoutFile ? Promise.resolve() : playSplash();

  let session = await window.void.authGetSession();
  if (!session) {
    await splashDone;
    session = await showLoginScreen(appEl);
  }
  state.session = session;

  // Load settings before building the UI that reads them, and apply the
  // theme/background immediately so there's no flash of the wrong look.
  state.settings = await window.void.settingsGet();
  applyAppearance(state.settings);
  applyAmbient(state.settings);

  // First run after sign-in: onboarding (theme etc.). Persist the choices.
  if (!state.settings.onboarded) {
    const choices = await showOnboarding(appEl, state.settings);
    if (Object.keys(choices).length > 0) {
      await window.void.settingsSet(choices);
      state.settings = await window.void.settingsGet();
      applyAppearance(state.settings);
    }
  }

  const layout = initLayout(appEl);
  const editor = initEditorArea(layout.tabbarEl, layout.editorHostEl);
  initSidebar(layout.sidebarEl);
  initTerminalPanel(layout.terminalEl);
  initAIPanel(layout.aiPanelEl, editor);
  initStatusbar(layout.statusbarEl);
  initPalette(editor);
  initPreview(editor);

  bus.on('open-settings', () => openSettingsTab(editor));
  initPresence();
  bus.on('open-blbots', () => void import('./blbots/index').then((m) => m.openBlBots(editor)));
  bus.on('open-extension', (ref) => void import('./extensions/details').then((m) => m.openExtensionDetails(editor, ref)));
  bus.on('session-changed', (s) => {
    if (s === null) window.location.reload();
  });
  // Live theme/background updates from Settings.
  bus.on('settings-changed', (s) => { applyAppearance(s); applyAmbient(s); });
  window.void.onMenu((action) => dispatch(action, editor));
  bindKeys(editor);

  bus.emit('session-changed', session);

  // Updates. Automatic (default): download + verify in the background and say
  // when it is ready; nothing interrupts work. Off: the old "Update" banner.
  let shownReady = '';
  const autoUpdate = (): void => {
    if (state.settings.autoUpdate === false) {
      void window.void.checkVersion().then(maybeShowUpdateBanner).catch(() => undefined);
      return;
    }
    void window.void.autoUpdate().then((r) => {
      if ((r.status === 'installed' || r.status === 'ready') && r.version && r.version !== shownReady) {
        shownReady = r.version;
        showUpdateReady(r.version, r.status);
      }
    }).catch(() => undefined); // offline / feed unreachable: try again later
  };
  if (state.settings.autoUpdate === false) {
    if (versionInfo) maybeShowUpdateBanner(versionInfo);
  } else {
    window.setTimeout(autoUpdate, 15_000); // let the editor finish loading first
  }
  // Look again every 2 hours for long-running sessions.
  window.setInterval(autoUpdate, 2 * 60 * 60 * 1000);

  // Monaco loads in the background so the first file click paints instantly.
  editor.prewarm();

  // The model list loads in the background, and refreshes on focus and every
  // minute so new BlazeAPI models show up without a restart.
  function applyModels(models: typeof state.models): void {
    state.models = models;
    // Reconcile the selected model against what this account can call: a
    // stale id would make every send fail.
    const twin = models.find((m) => m.kind === 'chat' && m.id !== state.settings.selectedModel && normModelId(m.id) === normModelId(state.settings.selectedModel));
    if (twin && !models.some((m) => m.id === state.settings.selectedModel)) {
      // Same model under the other routing's spelling (gpt-6-1-sol / gpt-6.1-sol).
      state.settings.selectedModel = twin.id;
      void window.void.settingsSet({ selectedModel: twin.id });
    }
    if (models.length > 0 && !models.some((m) => m.id === state.settings.selectedModel)) {
      // Never fall back to an image model; those are used through a tool.
      const chatModels = models.filter((m) => m.kind === 'chat');
      const usable = chatModels.find((m) => m.id === 'claude-sonnet-5') ?? chatModels[0];
      if (usable) {
        state.settings.selectedModel = usable.id;
        void window.void.settingsSet({ selectedModel: usable.id });
      }
    }
    bus.emit('models-changed', models);
  }
  void window.void.aiGetModels().then(applyModels).catch((e) => bus.emit('models-error', String(e)));

  // Pick up admin-side catalog edits without a restart: refresh (force, cache
  // bypass) when the window regains focus, and every 60s while focused.
  let lastRefresh = 0;
  const refreshModels = (): void => {
    if (Date.now() - lastRefresh < 5000) return; // debounce focus bursts
    lastRefresh = Date.now();
    void window.void.aiRefreshModels().then(applyModels).catch((e) => { if (state.models.length === 0) bus.emit('models-error', String(e)); });
  };
  window.addEventListener('focus', refreshModels);
  window.setInterval(() => { if (document.hasFocus()) refreshModels(); }, 60_000);

  // Restore project / open the popped-out file, then lift the splash.
  if (popoutFile) {
    if (popoutRoot) await openProject(popoutRoot).catch(() => undefined);
    await editor.openFile(popoutFile);
  } else {
    try {
      const recent = await window.void.getRecentProjects();
      if (recent.length > 0) {
        await window.void.fsReadTree(recent[0]);
        await openProject(recent[0]);
      }
    } catch {
      // Folder moved or deleted — start empty.
    }
  }
  await splashDone;
}

void boot().catch((err: unknown) => {
  // Surface a boot failure instead of dying silently after the splash.
  console.error('[blaze] boot failed:', err instanceof Error ? err.stack : err);
});
