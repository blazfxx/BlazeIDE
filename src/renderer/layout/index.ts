// Workbench layout: DOM skeleton, panel resizers, collapse toggles.
// Panel sizes load from settings and persist (debounced) after drags.

import { bus, el, escapeHtml, state } from '../state';
import { icon, iconGear } from '../icons/index';

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

  const sidebar = el('div', '');
  sidebar.id = 'sidebar';

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

  main.append(sidebar, sidebarResizer, center, aiResizer, aiPanel);

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
    sidebarEl: sidebar,
    tabbarEl: tabbar,
    editorHostEl: editorHost,
    aiPanelEl: aiPanel,
    terminalEl: terminal,
    statusbarEl: statusbar,
  };
}
