// Small shared confirm modal with an optional "Don't show again" checkbox.
// Resolves { confirmed, dontShowAgain }.

import { el, escapeHtml } from '../state';

export interface ConfirmResult {
  confirmed: boolean;
  dontShowAgain: boolean;
}

export interface ConfirmOptions {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  showDontAskAgain?: boolean;
}

export function confirmModal(opts: ConfirmOptions): Promise<ConfirmResult> {
  return new Promise((resolve) => {
    const overlay = document.getElementById('overlay-root');
    if (!overlay) {
      resolve({ confirmed: false, dontShowAgain: false });
      return;
    }

    const backdrop = el('div', 'modal-backdrop');
    const card = el('div', 'modal-card confirm-card');
    card.appendChild(el('div', 'confirm-title', escapeHtml(opts.title)));
    card.appendChild(el('div', 'confirm-message', escapeHtml(opts.message)));

    let dontShow = false;
    if (opts.showDontAskAgain) {
      const row = el('label', 'confirm-dontask');
      const sw = el('span', 'switch');
      sw.appendChild(el('span', 'knob'));
      row.appendChild(sw);
      row.appendChild(el('span', '', "Don't show again"));
      row.addEventListener('click', () => {
        dontShow = sw.classList.toggle('on');
      });
      card.appendChild(row);
    }

    const actions = el('div', 'form-actions confirm-actions');
    const confirmBtn = el('button', opts.danger ? 'btn-danger' : 'primary', opts.confirmLabel ?? 'Confirm');
    const cancelBtn = el('button', '', 'Cancel');
    actions.append(confirmBtn, cancelBtn);
    card.appendChild(actions);
    backdrop.appendChild(card);
    overlay.appendChild(backdrop);

    function close(confirmed: boolean): void {
      backdrop.remove();
      document.removeEventListener('keydown', onKey, true);
      resolve({ confirmed, dontShowAgain: confirmed && dontShow });
    }
    function onKey(e: KeyboardEvent): void {
      if (e.key === 'Escape') {
        e.stopPropagation();
        close(false);
      } else if (e.key === 'Enter') {
        e.stopPropagation();
        close(true);
      }
    }
    confirmBtn.addEventListener('click', () => close(true));
    cancelBtn.addEventListener('click', () => close(false));
    backdrop.addEventListener('mousedown', (e) => {
      if (e.target === backdrop) close(false);
    });
    document.addEventListener('keydown', onKey, true);
    confirmBtn.focus();
  });
}
