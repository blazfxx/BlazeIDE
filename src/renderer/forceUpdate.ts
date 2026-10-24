// Force-update screen: a full-screen, non-dismissable overlay shown when the
// backend reports this app version is below the minimum supported. The user
// can't proceed — only download the new version or quit.

import type { VersionInfo } from '../shared/types';

export function showForceUpdate(info: VersionInfo): void {
  // Remove anything already on screen; this takes over completely.
  const root = document.getElementById('app') || document.body;
  const overlay = document.createElement('div');
  overlay.className = 'force-update';
  overlay.innerHTML = `
    <div class="fu-card">
      <div class="fu-mark">
        <svg viewBox="0 0 32 32" fill="none" stroke="#fff" stroke-width="2.4" stroke-linejoin="miter">
          <path d="M 8 8 H 18 V 12 H 24 V 24 H 14 V 20 H 8 Z"/></svg>
      </div>
      <h1>Update required</h1>
      <p>A newer version of Blaze IDE is required to continue.
         You're on <b>${escapeHtml(info.current || 'an old build')}</b>; the latest is <b>${escapeHtml(info.latest)}</b>.</p>
      <button class="fu-btn" id="fuDownload">Download the latest version</button>
      <button class="fu-quit" id="fuQuit">Quit</button>
    </div>`;
  // Cover everything.
  Object.assign(overlay.style, {
    position: 'fixed', inset: '0', zIndex: '99999',
    display: 'grid', placeItems: 'center', background: '#000',
  });
  document.body.appendChild(overlay);

  overlay.querySelector('#fuDownload')?.addEventListener('click', () => {
    void window.void.openExternal(info.downloadUrl);
  });
  overlay.querySelector('#fuQuit')?.addEventListener('click', () => {
    window.close();
  });
  void root; // root intentionally left in place behind the opaque overlay
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c] as string));
}
