// Soft "update available" banner — shown when a newer version exists but the
// current one is still supported (not a forced update). Dismissable, and it
// stays dismissed for that version so it doesn't nag.

import type { VersionInfo } from '../shared/types';

const DISMISS_KEY = 'void.updateDismissed'; // stores the latest version dismissed

function cmpVersion(a: string, b: string): number {
  const pa = a.split('.').map((n) => parseInt(n, 10) || 0);
  const pb = b.split('.').map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < 3; i++) { if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) - (pb[i] || 0); }
  return 0;
}

export function maybeShowUpdateBanner(info: VersionInfo): void {
  if (document.querySelector('.update-banner')) return; // already showing
  if (info.mustUpdate) return;            // forced path handles this
  if (!info.current) return;              // unknown current → skip
  if (cmpVersion(info.current, info.latest) >= 0) return; // already current
  if (localStorage.getItem(DISMISS_KEY) === info.latest) return; // dismissed this one

  const bar = document.createElement('div');
  bar.className = 'update-banner';
  bar.innerHTML = `
    <span class="ub-dot"></span>
    <span class="ub-text">Blaze IDE <b>${escapeHtml(info.latest)}</b> is available — you're on ${escapeHtml(info.current)}.</span>
    <button class="ub-get" id="ubGet">Update</button>
    <button class="ub-x" id="ubX" aria-label="Dismiss">&times;</button>`;
  document.body.appendChild(bar);
  // slide in next frame
  requestAnimationFrame(() => bar.classList.add('show'));

  const close = (): void => { bar.classList.remove('show'); setTimeout(() => bar.remove(), 250); };
  const get = bar.querySelector<HTMLButtonElement>('#ubGet');
  const text = bar.querySelector<HTMLElement>('.ub-text');
  get?.addEventListener('click', () => {
    if (!get || !text) return;
    get.disabled = true;
    get.textContent = 'Updating…';
    const off = window.void.onUpdateProgress((done, total) => {
      text.textContent = total ? `Downloading update… ${Math.min(100, Math.round((done / total) * 100))}%` : 'Downloading update…';
    });
    window.void.installUpdate().then(
      () => off(), // the app restarts itself
      (e) => {
        off();
        text.textContent = String(e);
        get.disabled = false;
        get.textContent = 'Download';
        get.onclick = () => void window.void.openExternal(info.downloadUrl);
      },
    );
  });
  bar.querySelector('#ubX')?.addEventListener('click', () => {
    localStorage.setItem(DISMISS_KEY, info.latest); // don't nag again for this version
    close();
  });
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c] as string));
}
