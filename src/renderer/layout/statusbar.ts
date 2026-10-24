// Statusbar: branch, error/warning counts, language, line:col, balance, AI dot.

import { bus, el, escapeHtml, state } from '../state';
import { icon, iconBranch, iconError, iconWarning } from '../icons/index';
import { formatUsd } from '../account/index';

export function initStatusbar(host: HTMLElement): void {
  const branchEl = el('span', 'sb-item sb-branch');
  branchEl.style.display = 'none';
  const problemsEl = el('span', 'sb-item sb-problems');
  const right = el('span', 'sb-right');
  const langEl = el('span', 'sb-item sb-lang');
  const posEl = el('span', 'sb-item sb-pos');
  const aiDot = el('span', 'ai-dot');
  aiDot.title = 'AI idle';
  // BlazeAPI balance; click opens the account panel (top up, sign out).
  const balanceEl = el('span', 'sb-item sb-balance');
  balanceEl.style.display = 'none';
  balanceEl.style.cursor = 'pointer';
  balanceEl.title = 'BlazeAPI balance';
  balanceEl.addEventListener('click', () => bus.emit('open-account', undefined));
  right.append(langEl, posEl, balanceEl, aiDot);
  host.append(branchEl, problemsEl, right);

  renderProblems(0, 0);

  async function refreshUsage(): Promise<void> {
    const u = await window.void.authGetUsage().catch(() => null);
    if (!u) {
      balanceEl.style.display = 'none';
      return;
    }
    balanceEl.textContent = formatUsd(u.balanceUsd);
    balanceEl.classList.toggle('warn', u.balanceUsd < 0.5);
    balanceEl.style.display = '';
  }

  async function refreshBranch(): Promise<void> {
    if (!state.projectRoot) {
      branchEl.style.display = 'none';
      return;
    }
    try {
      const head = await window.void.fsTryReadFile(`${state.projectRoot}/.git/HEAD`);
      if (head === null) {
        branchEl.style.display = 'none';
        return;
      }
      const match = head.match(/ref: refs\/heads\/(.+)/);
      const branch = match ? match[1].trim() : head.trim().slice(0, 8);
      branchEl.innerHTML = `${icon(iconBranch, 'icon-12')} ${escapeHtml(branch)}`;
      branchEl.style.display = '';
    } catch {
      branchEl.style.display = 'none';
    }
  }

  function renderProblems(errors: number, warnings: number): void {
    problemsEl.innerHTML = `${icon(iconError, 'icon-12')} ${errors} ${icon(iconWarning, 'icon-12')} ${warnings}`;
  }

  bus.on('project-opened', () => void refreshBranch());
  window.void.onFsChanged(() => void refreshBranch());
  bus.on('markers-changed', ({ errors, warnings }) => renderProblems(errors, warnings));
  bus.on('editor-lang-changed', ({ lang }) => {
    langEl.textContent = lang;
  });
  bus.on('cursor-changed', ({ line, col }) => {
    posEl.textContent = `${line}:${col}`;
  });
  bus.on('active-file-changed', ({ path }) => {
    if (!path) {
      langEl.textContent = '';
      posEl.textContent = '';
    }
  });
  bus.on('ai-status', ({ streaming }) => {
    aiDot.classList.toggle('busy', streaming);
    aiDot.title = streaming ? 'AI streaming' : 'AI idle';
    // Refresh usage right after a request finishes (it just consumed some).
    if (!streaming) void refreshUsage();
  });
  bus.on('session-changed', () => void refreshUsage());
  void refreshUsage();
  window.setInterval(() => void refreshUsage(), 60_000);
}
