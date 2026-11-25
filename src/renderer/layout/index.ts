// Workbench layout: DOM skeleton, panel resizers, collapse toggles.
// Panel sizes load from settings and persist (debounced) after drags.

import { bus, el, escapeHtml, state } from '../state';
import { icon, iconBots, iconExtensions, iconFiles, iconGear, iconSearch } from '../icons/index';

export interface LayoutEls {
  sidebarEl: HTMLElement;
  tabbarEl: HTMLElement;
  editorHostEl: HTMLElement;
  aiPanelEl: HTMLElement;
  terminalEl: HTMLElement;
  statusbarEl: HTMLElement;
}

const MIN_SIDEBAR = 140;
const MAX_SIDEBAR = 420;
const MIN_AI = 240;
const MAX_AI = 560;
const MIN_TERM = 80;
const MAX_TERM_RATIO = 0.7;

const sizes = { sidebarWidth: 220, aiPanelWidth: 320, terminalHeight: 200 };
let persistTimer: number | null = null;

function setVar(name: string, px: number): void {
  document.documentElement.style.setProperty(name, `${px}px`);
}

function applySizes(): void {
  setVar('--sidebar-w', sizes.sidebarWidth);
  setVar('--ai-panel-w', sizes.aiPanelWidth);
  setVar('--terminal-h', sizes.terminalHeight);
}

function persistSizes(): void {
  if (persistTimer !== null) window.clearTimeout(persistTimer);
  persistTimer = window.setTimeout(() => {
    persistTimer = null;
    void window.void.settingsSet({ ...sizes });
  }, 400);
}

function makeResizer(cls: string, onDrag: (e: MouseEvent) => void): HTMLElement {
  const r = el('div', `resizer ${cls}`);
  r.addEventListener('mousedown', (down) => {
    down.preventDefault();
    const move = (e: MouseEvent): void => onDrag(e);
    const up = (): void => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      document.body.style.cursor = '';
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
    document.body.style.cursor = cls === 'resizer-term' ? 'row-resize' : 'col-resize';
  });
  return r;
}

export function initLayout(app: HTMLElement): LayoutEls {
  const titlebar = el('div', '');
  titlebar.id = 'titlebar';
  // Tauri moves the window when this element (not its children) is dragged.
  titlebar.setAttribute('data-tauri-drag-region', '');
  const titleText = el('span', 'title-text', 'Blaze IDE');

  // Settings gear, pinned top-right of the titlebar.
  const settingsBtn = el('button', 'icon-btn titlebar-settings', icon(iconGear));
  settingsBtn.title = window.void.platform() === 'mac' ? 'Settings (⌘,)' : 'Settings (Ctrl+,)';
  settingsBtn.addEventListener('click', () => bus.emit('open-settings', undefined));

  titlebar.append(titleText, settingsBtn);

  const main = el('div', '');
  main.id = 'main';

  // VS Code-style activity bar: one icon per view, Settings at the bottom.
  const activity = el('div', '');
  activity.id = 'activitybar';
  const sidebar = el('div', '');
  sidebar.id = 'sidebar';
  sidebar.dataset.view = 'files';
  const filesView = el('div', 'sb-view sb-files');
  const extView = el('div', 'sb-view sb-ext');
  sidebar.append(filesView, extView);
  let extBuilt = false;
  const actBtn = (svgStr: string, tip: string, view: string | null, onClick?: () => void): HTMLElement => {
    const b = el('button', 'act-btn', icon(svgStr));
    b.title = tip;
    if (view) b.dataset.view = view;
    b.addEventListener('click', () => {
      if (onClick) { onClick(); return; }
      if (!view) return;
      const collapsed = sidebar.classList.contains('collapsed');
      if (!collapsed && sidebar.dataset.view === view && view !== 'search') {
        bus.emit('toggle-sidebar', undefined);
        return;
      }
      if (collapsed) bus.emit('toggle-sidebar', undefined);
      sidebar.dataset.view = view === 'search' ? 'files' : view;
      if (view === 'search') bus.emit('sidebar-search', undefined);
      if (view === 'extensions' && !extBuilt) {
        extBuilt = true;
        const head = el('div', 'sb-view-head', 'Extensions');
        const body = el('div', 'sb-ext-body');
        extView.append(head, body);
        void import('../extensions/view').then((m) => m.buildExtensionsView(body));
      }
      markActive();
    });
    return b;
  };
  const acts = [
    actBtn(iconFiles, 'Explorer', 'files'),
    actBtn(iconSearch, 'Search', 'search'),
    actBtn(iconExtensions, 'Extensions', 'extensions'),
    actBtn(iconBots, 'BlBots: your AI bots and rooms', null, () => bus.emit('open-blbots', undefined)),
  ];
  const spacer = el('div', 'act-spacer');
  const actSettings = actBtn(iconGear, 'Settings', null, () => bus.emit('open-settings', undefined));
  activity.append(...acts, spacer, actSettings);
  const markActive = (): void => {
    const collapsed = sidebar.classList.contains('collapsed');
    for (const b of acts) b.classList.toggle('active', !collapsed && !!b.dataset.view && b.dataset.view === sidebar.dataset.view);
  };
  markActive();

  const center = el('div', '');
  center.id = 'center';
  const tabbar = el('div', '');
  tabbar.id = 'tabbar';
  const editorHost = el('div', '');
  editorHost.id = 'editor-host';
  center.append(tabbar, editorHost);

  const aiPanel = el('div', '');
  aiPanel.id = 'ai-panel';

  const sidebarResizer = makeResizer('resizer-sidebar', (e) => {
    sizes.sidebarWidth = Math.min(MAX_SIDEBAR, Math.max(MIN_SIDEBAR, e.clientX));
    setVar('--sidebar-w', sizes.sidebarWidth);
    persistSizes();
  });
  const aiResizer = makeResizer('resizer-ai', (e) => {
    sizes.aiPanelWidth = Math.min(MAX_AI, Math.max(MIN_AI, window.innerWidth - e.clientX));
    setVar('--ai-panel-w', sizes.aiPanelWidth);
    persistSizes();
  });

  main.append(activity, sidebar, sidebarResizer, center, aiResizer, aiPanel);

  const termResizer = makeResizer('resizer-term', (e) => {
    sizes.terminalHeight = Math.min(
      window.innerHeight * MAX_TERM_RATIO,
      Math.max(MIN_TERM, window.innerHeight - e.clientY - 24)
    );
    setVar('--terminal-h', sizes.terminalHeight);
    persistSizes();
  });

  const terminal = el('div', '');
  terminal.id = 'terminal-panel';
  terminal.classList.add('collapsed');
  termResizer.classList.add('collapsed');

  const statusbar = el('div', '');
  statusbar.id = 'statusbar';

  app.append(titlebar, main, termResizer, terminal, statusbar);

  if (!document.getElementById('overlay-root')) {
    document.body.appendChild(Object.assign(el('div', ''), { id: 'overlay-root' }));
  }

  bus.on('project-opened', ({ name }) => {
    titleText.innerHTML = `Blaze IDE <span class="title-sep">·</span> ${escapeHtml(name)}`;
  });

  bus.on('toggle-sidebar', () => {
    sidebar.classList.toggle('collapsed');
    sidebarResizer.classList.toggle('collapsed');
    markActive();
  });
  bus.on('toggle-ai-panel', () => {
    aiPanel.classList.toggle('collapsed');
    aiResizer.classList.toggle('collapsed');
  });
  bus.on('toggle-terminal', () => {
    terminal.classList.toggle('collapsed');
    termResizer.classList.toggle('collapsed');
  });
  bus.on('focus-ai-input', () => {
    aiPanel.classList.remove('collapsed');
    aiResizer.classList.remove('collapsed');
  });

  // Initial sizes come from settings (loaded into state before initLayout);
  // settings-tab edits arrive via settings-changed.
  sizes.sidebarWidth = state.settings.sidebarWidth;
  sizes.aiPanelWidth = state.settings.aiPanelWidth;
  sizes.terminalHeight = state.settings.terminalHeight;
  applySizes();
  bus.on('settings-changed', (s) => {
    if (
      s.sidebarWidth !== sizes.sidebarWidth ||
      s.aiPanelWidth !== sizes.aiPanelWidth ||
      s.terminalHeight !== sizes.terminalHeight
    ) {
      sizes.sidebarWidth = s.sidebarWidth;
      sizes.aiPanelWidth = s.aiPanelWidth;
      sizes.terminalHeight = s.terminalHeight;
      applySizes();
    }
  });

  return {
    sidebarEl: filesView,
    tabbarEl: tabbar,
    editorHostEl: editorHost,
    aiPanelEl: aiPanel,
    terminalEl: terminal,
    statusbarEl: statusbar,
  };
}
