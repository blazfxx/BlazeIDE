// Account modal: BlazeAPI email, balance, top up, sign out.
// Registers its bus listener on import (documented side effect).

import { BILLING_URL, SITE_BASE } from '../../shared/modelCatalog';
import { bus, el, escapeHtml, state } from '../state';
import { iconUser } from '../icons/index';

export function formatUsd(n: number): string {
  if (n >= 100) return `$${n.toFixed(0)}`;
  if (n >= 1) return `$${n.toFixed(2)}`;
  return `$${n.toFixed(n >= 0.01 ? 2 : 4)}`;
}

export function openAccountPanel(): void {
  const overlay = document.getElementById('overlay-root');
  if (!overlay || !state.session) return;
  if (overlay.querySelector('.modal-backdrop')) return;

  const backdrop = el('div', 'modal-backdrop');
  const card = el('div', 'modal-card account-card');

  const head = el('div', 'account-head');
  head.appendChild(el('span', 'icon icon-20', iconUser));
  head.appendChild(el('span', 'account-email', escapeHtml(state.session.userEmail || state.session.username)));
  card.appendChild(head);
  if (state.session.username) card.appendChild(el('div', 'account-user', `@${escapeHtml(state.session.username)}`));

  const balance = el('div', 'account-balance', 'Balance: <b>…</b>');
  card.appendChild(balance);
  void window.void.authGetUsage().then((usage) => {
    balance.innerHTML = usage ? `Balance: <b>${escapeHtml(formatUsd(usage.balanceUsd))}</b>` : 'Balance unavailable right now.';
  });

  const actions = el('div', 'account-actions');
  const topUp = el('button', 'primary', 'Top up balance');
  topUp.addEventListener('click', () => void window.void.openExternal(BILLING_URL));
  const dash = el('button', '', 'Open dashboard');
  dash.addEventListener('click', () => void window.void.openExternal(`${SITE_BASE}/dashboard`));
  const signOutBtn = el('button', 'btn-danger', 'Sign out');
  signOutBtn.addEventListener('click', () => {
    signOutBtn.disabled = true;
    void window.void.authLogout().finally(() => {
      close();
      bus.emit('session-changed', null);
    });
  });
  actions.append(topUp, dash, signOutBtn);
  card.appendChild(actions);

  backdrop.appendChild(card);
  overlay.appendChild(backdrop);

  function close(): void {
    backdrop.remove();
    document.removeEventListener('keydown', onKey, true);
  }
  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') close();
  }
  backdrop.addEventListener('mousedown', (e) => {
    if (e.target === backdrop) close();
  });
  document.addEventListener('keydown', onKey, true);
}

bus.on('open-account', () => openAccountPanel());
