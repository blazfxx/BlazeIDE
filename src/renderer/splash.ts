// Boot splash: calm and quick — the flame draws itself, "blaze" fades
// up beneath it, a thin copper line sweeps across, then the overlay lifts.

import { el } from './state';
import { brandMarkSvg, BRAND_ORANGE } from './icons/brandMark';

const ENTER_MS = 900;
const HOLD_MS = 350;
const FADE_MS = 320;

export function playSplash(): Promise<void> {
  return new Promise((resolve) => {
    const splash = el('div', 'splash');
    const inner = el('div', 'splash-inner');
    const mark = el('div', 'splash-mark', brandMarkSvg(44, BRAND_ORANGE, 1.8));
    const word = el('div', 'splash-word', 'blaze');
    const line = el('div', 'splash-line');
    inner.append(mark, word, line);
    splash.appendChild(inner);
    document.body.appendChild(splash);

    // Kick the enter animations on the next frame so transitions apply.
    requestAnimationFrame(() => splash.classList.add('in'));

    window.setTimeout(() => {
      splash.classList.add('lifting');
      window.setTimeout(() => {
        splash.remove();
        resolve();
      }, FADE_MS);
    }, ENTER_MS + HOLD_MS);
  });
}
