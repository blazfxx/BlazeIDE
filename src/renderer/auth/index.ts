// Full-window sign-in screen. Sign-in/up happens on blazeapi.org in the
// browser; the Rust side catches the redirect on 127.0.0.1 and resolves
// authSignIn with the session.

import type { SessionPublic } from '../../shared/types';
import { el, escapeHtml } from '../state';
import { brandMarkSvg, BRAND_ORANGE } from '../icons/brandMark';

export function showLoginScreen(root: HTMLElement): Promise<SessionPublic> {
  return new Promise((resolve) => {
    const screen = el('div', 'login-screen');
    const card = el('div', 'login-card');

    card.appendChild(el('div', 'login-mark', brandMarkSvg(44, BRAND_ORANGE, 1.8)));
    card.appendChild(el('div', 'login-word', 'blaze'));
    const subtitle = el('div', 'login-subtitle', 'Sign in with your BlazeAPI account');
    card.appendChild(subtitle);

    const connectBtn = el('button', 'primary login-submit', 'Sign in with BlazeAPI');
    const hint = el('div', 'login-hint', 'Opens blazeapi.org in your browser. No account yet? You can create one there.');
    const error = el('div', 'field-error');
    error.style.display = 'none';
    const cancelBtn = el('button', 'login-link-btn', 'Cancel');
    cancelBtn.style.display = 'none';

    card.append(connectBtn, hint, error, cancelBtn);
    screen.appendChild(card);
    root.appendChild(screen);

    let busy = false;

    function setIdle(msg: string | null): void {
      busy = false;
      connectBtn.disabled = false;
      connectBtn.textContent = 'Sign in with BlazeAPI';
      subtitle.textContent = 'Sign in with your BlazeAPI account';
      cancelBtn.style.display = 'none';
      error.textContent = msg ? escapeHtml(msg) : '';
      error.style.display = msg ? 'block' : 'none';
    }

    connectBtn.addEventListener('click', () => {
      if (busy) return;
      busy = true;
      connectBtn.disabled = true;
      connectBtn.textContent = 'Waiting for the browser…';
      subtitle.textContent = 'Finish signing in on blazeapi.org';
      cancelBtn.style.display = '';
      error.style.display = 'none';
      window.void
        .authSignIn()
        .then((session) => {
          screen.style.opacity = '0';
          window.setTimeout(() => { screen.remove(); resolve(session); }, 150);
        })
        .catch((e: unknown) => {
          const msg = typeof e === 'string' ? e : e instanceof Error ? e.message : 'Sign-in failed';
          setIdle(/cancelled/i.test(msg) ? null : msg);
        });
    });

    cancelBtn.addEventListener('click', () => {
      void window.void.authCancel();
    });
  });
}
