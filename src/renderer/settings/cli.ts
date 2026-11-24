// Settings > Command line: put the bundled `blaze` CLI on the PATH.

import { el } from '../state';
import { settingBlock } from './controls';

export function buildCliSection(): HTMLElement[] {
  const body = el('div', 'cli-install');
  const line = el('div', 'set-inline-note', 'Checking…');
  line.style.maxWidth = 'none';
  line.style.whiteSpace = 'normal';
  const btn = el('button', '', 'Install the blaze command') as HTMLButtonElement;
  btn.disabled = true;
  const out = el('div', 'set-inline-note');
  out.style.maxWidth = 'none';
  out.style.whiteSpace = 'normal';
  body.append(line, btn, out);

  const refresh = (): void => {
    void window.void.cliStatus().then((st) => {
      if (!st.available) {
        line.textContent = 'This build does not include the blaze command. Get it from blazeapi.org/ide.';
        btn.disabled = true;
        return;
      }
      line.textContent = st.installed ? 'Installed. Run `blaze` in any terminal.' : 'Not on your PATH yet.';
      btn.textContent = st.installed ? 'Reinstall' : 'Install the blaze command';
      btn.disabled = false;
    });
  };
  btn.addEventListener('click', () => {
    btn.disabled = true;
    out.textContent = '';
    void window.void
      .cliInstall()
      .then((msg) => { out.textContent = msg; })
      .catch((e: unknown) => { out.textContent = e instanceof Error ? e.message : String(e); })
      .finally(refresh);
  });
  refresh();
  return [
    settingBlock(
      'blaze in the terminal',
      'The same agent, models and chats from any terminal: `blaze` opens a full-screen chat in the current folder, `blaze -p "..."` runs one prompt. Chats are shared with the IDE.',
      body
    ),
  ];
}
