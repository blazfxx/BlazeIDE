// First-run onboarding shown right after sign-in: look, AI features, privacy.
// Persists the choices and marks `onboarded` so it only appears once. Resolves
// when finished or skipped.

import type { AppSettings, Theme } from '../../shared/types';
import { el } from '../state';
import { brandMarkSvg, BRAND_ORANGE } from '../icons/brandMark';
import { applyAppearance } from '../appearance';

const STEPS = 3;

export function showOnboarding(root: HTMLElement, settings: AppSettings): Promise<Partial<AppSettings>> {
  return new Promise((resolve) => {
    let theme: Theme = settings.theme;
    const chosen: Partial<AppSettings> = { aiAutocomplete: settings.aiAutocomplete, aiMediaGen: false, analytics: false };

    const screen = el('div', 'onb-screen');
    const card = el('div', 'onb-card');
    const body = el('div', 'onb-body');
    card.appendChild(body);
    screen.appendChild(card);
    root.appendChild(screen);

    const finish = (): void => {
      chosen.theme = theme;
      chosen.onboarded = true;
      screen.style.opacity = '0';
      window.setTimeout(() => { screen.remove(); resolve(chosen); }, 160);
    };

    function header(step: number, title: string, sub: string): void {
      body.textContent = '';
      body.appendChild(el('div', 'onb-mark', brandMarkSvg(40, BRAND_ORANGE, 1.8)));
      body.appendChild(el('div', 'onb-title', title));
      body.appendChild(el('div', 'onb-sub', sub));
      const dots = el('div', 'onb-dots');
      for (let i = 0; i < STEPS; i++) dots.appendChild(el('span', `onb-dot${i === step ? ' on' : ''}`));
      body.appendChild(dots);
    }

    function switchRow(label: string, hint: string, on: boolean, set: (v: boolean) => void): HTMLElement {
      const row = el('div', 'onb-toggle-row');
      const text = el('div', 'onb-toggle-text');
      text.appendChild(el('div', 'onb-toggle-label', label));
      text.appendChild(el('div', 'onb-toggle-hint', hint));
      const sw = el('label', `switch${on ? ' on' : ''}`);
      sw.appendChild(el('span', 'knob'));
      sw.addEventListener('click', () => set(sw.classList.toggle('on')));
      row.append(text, sw);
      return row;
    }

    // --- 1. Look ---
    function renderLook(): void {
      header(0, 'Welcome to Blaze IDE', "Let's set the look. You can change all of this later in Settings.");
      body.appendChild(el('div', 'onb-label', 'Theme'));
      const tiles = el('div', 'onb-themes');
      const tileFor = (value: Theme, name: string): HTMLElement => {
        const tile = el('button', `onb-theme onb-theme-${value}${theme === value ? ' selected' : ''}`);
        tile.innerHTML =
          `<span class="onb-swatch onb-swatch-${value}"><span class="s-bar"></span><span class="s-body"></span></span>` +
          `<span class="onb-theme-name">${name}</span>`;
        tile.addEventListener('click', () => {
          theme = value;
          applyAppearance({ ...settings, theme: value }); // live preview
          tiles.querySelectorAll('.onb-theme').forEach((t) => t.classList.remove('selected'));
          tile.classList.add('selected');
        });
        return tile;
      };
      tiles.append(tileFor('dark', 'Dark'), tileFor('light', 'Light'));
      body.appendChild(tiles);
      actions('Skip setup', finish, 'Continue', renderAI);
    }

    // --- 2. AI features ---
    function renderAI(): void {
      header(1, 'AI features', 'All of it runs on your BlazeAPI account. Change any of these later in Settings > AI.');
      body.appendChild(switchRow('Autocomplete', 'Ghost-text suggestions as you type; Tab accepts. Uses a very cheap fast model.', chosen.aiAutocomplete === true, (v) => { chosen.aiAutocomplete = v; }));
      body.appendChild(switchRow('Image generation', 'Let the AI agent generate images for your projects. Billed per image, so it starts off.', false, (v) => { chosen.aiMediaGen = v; }));
      actions('Back', renderLook, 'Continue', renderPrivacy);
    }

    // --- 3. Privacy / analytics: two equal choices, no pre-selected default ---
    function renderPrivacy(): void {
      header(2, 'Help improve Blaze IDE?', 'Share anonymous usage counts so we know what to fix first.');
      const list = el('ul', 'onb-privacy');
      for (const t of [
        'Sent: a random install id, the app version, your platform, and event names (opened, signed in, AI request with the BlazeAPI model id, updated).',
        'Never sent: your files, prompts, file paths or custom providers.',
        'You can change this any time in Settings > Privacy.',
      ]) list.appendChild(el('li', '', t));
      body.appendChild(list);
      const row = el('div', 'onb-actions');
      const no = el('button', 'onb-choice', 'No thanks');
      const yes = el('button', 'onb-choice', 'Share anonymous usage');
      no.addEventListener('click', () => { chosen.analytics = false; finish(); });
      yes.addEventListener('click', () => { chosen.analytics = true; finish(); });
      row.append(no, yes);
      body.appendChild(row);
      const back = el('button', 'onb-skip', 'Back');
      back.addEventListener('click', renderAI);
      body.appendChild(back);
    }

    function actions(leftLabel: string, left: () => void, rightLabel: string, right: () => void): void {
      const row = el('div', 'onb-actions');
      const l = el('button', 'onb-skip', leftLabel);
      const r = el('button', 'primary onb-done', rightLabel);
      l.addEventListener('click', left);
      r.addEventListener('click', right);
      row.append(l, r);
      body.appendChild(row);
    }

    renderLook();
  });
}
