// Terminal panel: tabbed xterm.js sessions backed by node-pty in the main
// process. Lazy — no PTY or xterm instance exists until the panel is first
// shown. PTY data/exit listeners are registered once and routed by session id.

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { state, bus, el } from '../state';
import { icon, iconClose, iconPlus, iconChevronDown, iconTerminal } from '../icons/index';

export interface TerminalPanel {
  toggle(): void;
  show(): void;
  isVisible(): boolean;
}

const FONT_FAMILY = 'JetBrains Mono, Fira Code, Cascadia Code, Menlo, Consolas, monospace';

// Flat palette mapped from base.css variables. magenta intentionally reuses
// red (no purple anywhere), blue and cyan reuse the copper accent.
const TERM_THEME = {
  background: '#0a0a0a',
  foreground: '#f5f5f5',
  cursor: '#ffffff',
  selectionBackground: '#262626',
  black: '#000000',
  red: '#e6e6e6',
  green: '#c8c8c8',
  yellow: '#a8a8a8',
  blue: '#ffffff',
  magenta: '#e6e6e6',
  cyan: '#d4d4d4',
  white: '#f5f5f5',
  brightBlack: '#4a4a4a',
  brightRed: '#e6e6e6',
  brightGreen: '#c8c8c8',
  brightYellow: '#a8a8a8',
  brightBlue: '#d4d4d4',
  brightMagenta: '#e6e6e6',
  brightCyan: '#d4d4d4',
  brightWhite: '#ffffff',
};

const PANEL_CSS = `
.term-panel { flex-direction: column; background: var(--bg-panel); border-top: 1px solid var(--border); overflow: hidden; }
.term-header { display: flex; align-items: center; gap: 4px; height: 30px; min-height: 30px; padding: 0 8px; border-bottom: 1px solid var(--border); }
.term-label { display: flex; align-items: center; gap: 6px; color: var(--text-secondary); font-size: 11px; text-transform: uppercase; letter-spacing: 0.5px; margin-right: 8px; }
.term-tabs { display: flex; align-items: center; gap: 2px; flex: 1; min-width: 0; overflow-x: auto; }
.term-tab { display: flex; align-items: center; gap: 6px; padding: 2px 4px 2px 8px; border-radius: var(--radius-sm); font-size: 12px; color: var(--text-secondary); cursor: pointer; white-space: nowrap; }
.term-tab:hover { background: var(--bg-hover); }
.term-tab.active { background: var(--bg-active); color: var(--text-primary); }
.term-tab.exited { color: var(--text-disabled); font-style: italic; }
.term-tab-close { display: inline-flex; padding: 1px; border-radius: 3px; color: var(--text-secondary); }
.term-tab-close:hover { background: var(--bg-hover); color: var(--text-primary); }
.term-btn { display: inline-flex; align-items: center; background: transparent; border: none; padding: 3px; border-radius: var(--radius-sm); color: var(--text-secondary); cursor: pointer; }
.term-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.term-body { position: relative; flex: 1; min-height: 0; }
.term-wrap { position: absolute; inset: 0; padding: 4px 0 0 8px; }
.term-wrap .xterm, .term-wrap .xterm-viewport, .term-wrap .xterm-screen { height: 100%; }
.term-error { position: absolute; top: 6px; left: 8px; right: 8px; z-index: 2; background: var(--bg-elevated); border: 1px solid var(--accent-error); color: var(--accent-error); padding: 4px 8px; border-radius: var(--radius-sm); font-size: 12px; }
`;

function injectStyles(): void {
  if (document.getElementById('term-panel-styles')) return;
  const style = document.createElement('style');
  style.id = 'term-panel-styles';
  style.textContent = PANEL_CSS;
  document.head.append(style);
}

interface TermSession {
  id: string;
  title: string;
  term: Terminal;
  fit: FitAddon;
  wrapEl: HTMLElement;
  tabEl: HTMLElement;
  labelEl: HTMLElement;
  exited: boolean;
}

export function initTerminalPanel(root: HTMLElement): TerminalPanel {
  injectStyles();

  const sessions = new Map<string, TermSession>();
  let activeId: string | null = null;
  let visible = false;
  let counter = 0;
  let creating = false;

  root.classList.add('term-panel');
  root.style.display = 'none';

  const header = el('div', 'term-header');
  const label = el('div', 'term-label', `${icon(iconTerminal, 'icon-12')}<span>Terminal</span>`);
  const tabsEl = el('div', 'term-tabs');
  const newBtn = el('button', 'term-btn', icon(iconPlus, 'icon-12'));
  newBtn.title = 'New terminal';
  const collapseBtn = el('button', 'term-btn', icon(iconChevronDown, 'icon-12'));
  collapseBtn.title = 'Hide terminal panel';
  header.append(label, tabsEl, newBtn, collapseBtn);

  const body = el('div', 'term-body');
  const errorEl = el('div', 'term-error');
  errorEl.style.display = 'none';
  body.append(errorEl);
  root.append(header, body);

  function showError(message: string): void {
    errorEl.textContent = message;
    errorEl.style.display = 'block';
  }

  function clearError(): void {
    errorEl.textContent = '';
    errorEl.style.display = 'none';
  }

  function activeSession(): TermSession | null {
    return activeId ? (sessions.get(activeId) ?? null) : null;
  }

  function refit(s: TermSession | null): void {
    if (!s || !visible || s.id !== activeId || body.clientHeight === 0) return;
    try {
      s.fit.fit();
    } catch {
      return;
    }
    if (!s.exited) window.void.termResize(s.id, s.term.cols, s.term.rows);
  }

  function activate(id: string): void {
    activeId = id;
    for (const s of sessions.values()) {
      const isActive = s.id === id;
      s.wrapEl.style.display = isActive ? 'block' : 'none';
      s.tabEl.classList.toggle('active', isActive);
    }
    const s = activeSession();
    refit(s);
    s?.term.focus();
  }

  function markExited(s: TermSession): void {
    if (s.exited) return;
    s.exited = true;
    s.tabEl.classList.add('exited');
    s.labelEl.textContent = `${s.title} (exited)`;
  }

  function hidePanel(): void {
    visible = false;
    root.style.display = 'none';
  }

  function closeSession(s: TermSession): void {
    if (!s.exited) window.void.termKill(s.id);
    s.term.dispose();
    s.wrapEl.remove();
    s.tabEl.remove();
    sessions.delete(s.id);
    if (activeId !== s.id) return;
    activeId = null;
    const rest = [...sessions.values()];
    const next = rest[rest.length - 1];
    if (next) activate(next.id);
    else hidePanel();
  }

  async function createSession(): Promise<void> {
    if (creating) return;
    creating = true;
    counter += 1;
    const title = `zsh ${counter}`;
    try {
      const id = await window.void.termCreate(state.projectRoot ?? '');
      clearError();

      const wrapEl = el('div', 'term-wrap');
      body.append(wrapEl);

      const term = new Terminal({
        theme: TERM_THEME,
        fontFamily: FONT_FAMILY,
        fontSize: state.settings.terminalFontSize,
        cursorBlink: true,
        scrollback: 5000,
        macOptionIsMeta: true,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(wrapEl);
      term.onData((data) => {
        window.void.termWrite(id, data);
      });

      const tabEl = el('div', 'term-tab');
      const labelEl = el('span', 'term-tab-label');
      labelEl.textContent = title;
      const closeEl = el('span', 'term-tab-close', icon(iconClose, 'icon-12'));
      closeEl.title = 'Close terminal';
      tabEl.append(labelEl, closeEl);
      tabsEl.append(tabEl);

      const session: TermSession = { id, title, term, fit, wrapEl, tabEl, labelEl, exited: false };
      sessions.set(id, session);

      tabEl.addEventListener('click', () => activate(id));
      closeEl.addEventListener('click', (ev) => {
        ev.stopPropagation();
        closeSession(session);
      });

      activate(id);
    } catch (err) {
      counter -= 1;
      showError(`Failed to start terminal: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      creating = false;
    }
  }

  function showPanel(autoCreate: boolean): void {
    if (!visible) {
      visible = true;
      root.style.display = 'flex';
    }
    if (sessions.size === 0) {
      if (autoCreate) void createSession();
      return;
    }
    const s = activeSession();
    refit(s);
    s?.term.focus();
  }

  // PTY push events: registered exactly once, routed to sessions by id.
  window.void.onTermData((id, data) => {
    sessions.get(id)?.term.write(data);
  });
  window.void.onTermExit((id) => {
    const s = sessions.get(id);
    if (s) markExited(s);
  });

  const resizeObserver = new ResizeObserver(() => refit(activeSession()));
  resizeObserver.observe(body);

  newBtn.addEventListener('click', () => {
    showPanel(false);
    void createSession();
  });
  collapseBtn.addEventListener('click', () => hidePanel());

  bus.on('toggle-terminal', () => {
    if (visible) hidePanel();
    else showPanel(true);
  });
  bus.on('new-terminal', () => {
    showPanel(false);
    void createSession();
  });
  bus.on('settings-changed', (s) => {
    for (const session of sessions.values()) {
      session.term.options.fontSize = s.terminalFontSize;
    }
    refit(activeSession());
  });

  return {
    toggle(): void {
      if (visible) hidePanel();
      else showPanel(true);
    },
    show(): void {
      showPanel(true);
    },
    isVisible(): boolean {
      return visible;
    },
  };
}
