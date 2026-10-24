// Chat controls bar: a context-usage meter (estimated tokens vs the model's
// context window) plus New chat / History / Compact actions. Sits between the
// model selector header and the chat list. Talks to the controller; refreshes
// the history list from the ai-chats-changed bus event.

import type { ChatSummary } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { icon, iconCompact, iconHistory, iconPlus, iconTrash } from '../icons/index';
import { activeContextWindow } from './chatStore';
import type { Controller } from './controller';

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}k`;
  return String(n);
}

function timeAgo(ts: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

export function initChatControls(host: HTMLElement, controller: Controller): void {
  const bar = el('div', 'ai-chat-controls');

  // --- context meter ---
  const meter = el('div', 'ctx-meter');
  meter.title = 'Conversation context used';
  const meterFill = el('div', 'ctx-meter-fill');
  const meterBar = el('div', 'ctx-meter-bar');
  meterBar.appendChild(meterFill);
  const meterLabel = el('span', 'ctx-meter-label', '0 / 0');
  meter.append(meterBar, meterLabel);

  // --- actions ---
  const actions = el('div', 'ai-chat-actions');
  const newBtn = el('button', 'chat-ctl-btn', icon(iconPlus, 'icon-12'));
  newBtn.title = 'New chat';
  const historyBtn = el('button', 'chat-ctl-btn', icon(iconHistory, 'icon-12'));
  historyBtn.title = 'Chat history';
  const compactBtn = el('button', 'chat-ctl-btn', icon(iconCompact, 'icon-12'));
  compactBtn.title = 'Compact conversation (summarize to free context)';
  actions.append(newBtn, historyBtn, compactBtn);

  bar.append(meter, actions);
  host.appendChild(bar);

  let lastTokens = 0;
  function setMeter(tokens: number, ctx: number): void {
    lastTokens = tokens;
    const pct = ctx > 0 ? Math.min(100, (tokens / ctx) * 100) : 0;
    meterFill.style.width = `${pct}%`;
    meterFill.classList.toggle('warn', pct >= 70 && pct < 90);
    meterFill.classList.toggle('danger', pct >= 90);
    meterLabel.textContent = `${fmtTokens(tokens)} / ${fmtTokens(ctx)} · ${Math.round(pct)}%`;
  }
  setMeter(0, activeContextWindow());

  bus.on('ai-context', ({ tokens, contextWindow }) => setMeter(tokens, contextWindow));
  // Switching model changes the denominator; keep the current token count.
  bus.on('models-changed', () => setMeter(lastTokens, activeContextWindow()));

  newBtn.addEventListener('click', () => void controller.newChat());
  compactBtn.addEventListener('click', () => void controller.compact());

  // --- history dropdown ---
  let panel: HTMLElement | null = null;
  function closePanel(): void {
    panel?.remove();
    panel = null;
    document.removeEventListener('mousedown', onAway, true);
  }
  function onAway(e: MouseEvent): void {
    if (panel && !panel.contains(e.target as Node) && e.target !== historyBtn) closePanel();
  }

  async function openPanel(): Promise<void> {
    if (panel) { closePanel(); return; }
    const root = state.projectRoot;
    panel = el('div', 'dropdown chat-history');
    if (!root) {
      panel.appendChild(el('div', 'chat-history-empty', 'Open a folder to save chats.'));
    } else {
      panel.appendChild(el('div', 'chat-history-loading', 'Loading…'));
    }
    bar.appendChild(panel);
    document.addEventListener('mousedown', onAway, true);
    if (!root) return;

    let chats: ChatSummary[] = [];
    try {
      chats = await window.void.chatList(root);
    } catch {
      // ignore
    }
    if (!panel) return; // closed while loading
    panel.textContent = '';
    if (chats.length === 0) {
      panel.appendChild(el('div', 'chat-history-empty', 'No saved chats yet.'));
      return;
    }
    for (const c of chats) {
      const row = el('div', 'chat-history-row');
      const main = el('div', 'chat-history-main');
      main.appendChild(el('div', 'chat-history-title', escapeHtml(c.title)));
      main.appendChild(el('div', 'chat-history-meta', `${c.messageCount} msg · ${timeAgo(c.updatedAt)}`));
      main.addEventListener('click', () => {
        closePanel();
        void controller.loadChat(c.id);
      });
      const del = el('button', 'chat-history-del', icon(iconTrash, 'icon-12'));
      del.title = 'Delete chat';
      del.addEventListener('click', async (e) => {
        e.stopPropagation();
        await window.void.chatDelete(root, c.id).catch(() => undefined);
        // If this was the chat we're currently editing, detach it so the
        // controller doesn't silently re-save it (which made deletes reappear).
        controller.forgetChat(c.id);
        row.remove();
        if (panel && panel.querySelectorAll('.chat-history-row').length === 0) {
          panel.appendChild(el('div', 'chat-history-empty', 'No saved chats yet.'));
        }
      });
      row.append(main, del);
      panel.appendChild(row);
    }
  }

  historyBtn.addEventListener('click', () => void openPanel());
}
