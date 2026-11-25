// Settings > Extensions: a small marketplace (Open VSX) plus the installed list.
// Blaze IDE runs the declarative parts of VS Code extensions (colour themes and
// snippets). An extension's own code needs the VS Code extension host, which we
// deliberately don't ship, so such parts are listed as "not supported".

import type { InstalledExt, MarketItem } from '../../shared/types';
import { themeKey } from '../extensions/apply';
import { bus, el, escapeHtml, state } from '../state';
import { apply, selectInput, settingBlock } from './controls';

const CATEGORIES: Array<{ label: string; value: string }> = [
  { label: 'All', value: '' },
  { label: 'Themes', value: 'Themes' },
  { label: 'Snippets', value: 'Snippets' },
];

function compact(n: number): string {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n);
}

function summary(e: InstalledExt): string {
  const parts: string[] = [];
  if (e.themes.length) parts.push(`${e.themes.length} theme${e.themes.length === 1 ? '' : 's'}`);
  if (e.snippets.length) parts.push(`${e.snippets.length} snippet set${e.snippets.length === 1 ? '' : 's'}`);
  return parts.length ? `Provides ${parts.join(' and ')}` : 'Nothing in this extension can run in Blaze IDE';
}

export function buildExtensionsSection(): HTMLElement[] {
  const themeHost = el('div', 'ext-theme');
  const installedHost = el('div', 'ext-list');
  const marketHost = el('div', 'ext-market');
  const status = el('div', 'ext-status');
  let installed: InstalledExt[] = [];
  const busy = new Set<string>();

  const changed = (): void => bus.emit('extensions-changed', undefined);

  async function refreshInstalled(): Promise<void> {
    installed = await window.void.extList().catch(() => [] as InstalledExt[]);
    renderInstalled();
    renderThemePicker();
  }

  function renderThemePicker(): void {
    themeHost.textContent = '';
    const options = [{ value: '', label: 'Blaze default' }];
    for (const e of installed) for (const t of e.themes) options.push({ value: themeKey(e.id, t.label), label: `${t.label} (${e.displayName})` });
    const current = options.some((o) => o.value === state.settings.editorTheme) ? state.settings.editorTheme : '';
    themeHost.appendChild(selectInput(options, current, (v) => void apply({ editorTheme: v })));
  }

  function renderInstalled(): void {
    installedHost.textContent = '';
    if (installed.length === 0) {
      installedHost.appendChild(el('div', 'ext-empty', 'No extensions installed yet. Search below.'));
      return;
    }
    for (const e of installed) {
      const row = el('div', 'prov-row ext-row');
      const avatar = el('div', 'ext-avatar', escapeHtml((e.displayName[0] ?? '?').toUpperCase()));
      if (e.icon && e.icon.startsWith('data:image/')) {
        const img = document.createElement('img');
        img.alt = '';
        img.addEventListener('load', () => { avatar.textContent = ''; avatar.classList.add('has-icon'); avatar.appendChild(img); });
        img.src = e.icon;
      }
      row.appendChild(avatar);
      const text = el('div', 'prov-row-text');
      text.appendChild(el('div', 'prov-row-name', `${escapeHtml(e.displayName)} <span class="ext-ver">${escapeHtml(e.version)}</span>`));
      text.appendChild(el('div', 'prov-row-sub', escapeHtml(summary(e))));
      if (e.unsupported.length) text.appendChild(el('div', 'prov-row-sub ext-warn', escapeHtml(`Not supported: ${e.unsupported.join(', ')}`)));
      const del = el('button', 'btn-danger', 'Uninstall');
      del.addEventListener('click', () => {
        void window.void.extUninstall(e.id).then(async () => {
          if (e.themes.some((t) => themeKey(e.id, t.label) === state.settings.editorTheme)) await apply({ editorTheme: '' });
          changed();
          await refreshInstalled();
        }, (err) => { status.textContent = String(err); });
      });
      row.append(text, del);
      installedHost.appendChild(row);
    }
  }

  // ---- marketplace ----
  const search = el('input', 'prov-input') as HTMLInputElement;
  search.type = 'search';
  search.placeholder = 'Search extensions (Open VSX)';
  search.spellcheck = false;
  let category = '';
  let items: MarketItem[] = [];
  let total = 0;
  let seq = 0;
  let timer: number | undefined;

  const chips = el('div', 'dd-kinds ext-chips');
  const chipEls = CATEGORIES.map((c) => {
    const b = el('button', 'dd-kind', escapeHtml(c.label));
    b.addEventListener('click', () => { category = c.value; markChips(); void run(false); });
    chips.appendChild(b);
    return b;
  });
  function markChips(): void {
    CATEGORIES.forEach((c, i) => chipEls[i].classList.toggle('active', c.value === category));
  }
  markChips();

  async function run(more: boolean): Promise<void> {
    const mine = ++seq;
    if (!more) { items = []; total = 0; }
    status.className = 'ext-status';
    status.textContent = 'Searching…';
    try {
      const page = await window.void.extSearch(search.value, category || undefined, more ? items.length : 0);
      if (mine !== seq) return; // a newer search replaced this one
      items = more ? [...items, ...page.items] : page.items;
      total = page.total;
      status.textContent = items.length === 0 ? 'No extensions found.' : '';
    } catch (e) {
      if (mine !== seq) return;
      status.className = 'ext-status err';
      status.textContent = String(e);
    }
    renderMarket();
  }

  function renderMarket(): void {
    marketHost.textContent = '';
    const have = new Set(installed.map((e) => e.id));
    for (const it of items) {
      const row = el('div', 'prov-row ext-row');
      const avatar = el('div', 'ext-avatar', escapeHtml((it.displayName[0] ?? '?').toUpperCase()));
      if (it.iconUrl) {
        const img = document.createElement('img');
        img.alt = '';
        img.referrerPolicy = 'no-referrer';
        img.addEventListener('load', () => { avatar.textContent = ''; avatar.classList.add('has-icon'); avatar.appendChild(img); });
        img.src = it.iconUrl;
      }
      const text = el('div', 'prov-row-text');
      const meta = [it.namespace, `${compact(it.downloads)} installs`, it.rating ? `${it.rating.toFixed(1)}★` : ''].filter(Boolean).join(' · ');
      text.appendChild(el('div', 'prov-row-name', `${escapeHtml(it.displayName)}${it.verified ? ' <span class="ext-ver" title="Verified publisher">✓</span>' : ''}`));
      text.appendChild(el('div', 'prov-row-sub', escapeHtml(it.description || meta)));
      if (it.description) text.appendChild(el('div', 'prov-row-sub', escapeHtml(meta)));
      const btn = el('button', have.has(it.id) ? '' : 'primary', have.has(it.id) ? 'Installed' : busy.has(it.id) ? 'Installing…' : 'Install') as HTMLButtonElement;
      btn.disabled = have.has(it.id) || busy.has(it.id);
      btn.addEventListener('click', () => {
        busy.add(it.id);
        renderMarket();
        void window.void.extInstall(it.namespace, it.name).then(
          async (ext) => {
            busy.delete(it.id);
            changed();
            await refreshInstalled();
            renderMarket();
            const first = ext.themes[0];
            status.className = 'ext-status';
            status.textContent = `Installed ${ext.displayName}. ${summary(ext)}${ext.unsupported.length ? `. Not supported: ${ext.unsupported.join(', ')}` : ''}.`;
            if (first && !state.settings.editorTheme) status.textContent += ' Pick it under “Editor colour theme”.';
          },
          (err) => { busy.delete(it.id); status.className = 'ext-status err'; status.textContent = `${it.displayName}: ${String(err)}`; renderMarket(); },
        );
      });
      row.append(avatar, text, btn);
      marketHost.appendChild(row);
    }
    if (items.length > 0 && items.length < total) {
      const more = el('button', '', 'Show more');
      more.addEventListener('click', () => void run(true));
      marketHost.appendChild(more);
    }
  }

  search.addEventListener('input', () => { window.clearTimeout(timer); timer = window.setTimeout(() => void run(false), 300); });

  const market = el('div', 'ext-market-wrap');
  market.append(search, chips, status, marketHost);

  void refreshInstalled();
  void run(false);

  return [
    settingBlock('Editor colour theme', 'Use a colour theme from an installed extension. The app chrome keeps its own look.', themeHost),
    settingBlock('Installed', 'Blaze IDE runs the themes and snippets in VS Code extensions. Extensions that need their own code (language servers, commands, debuggers) cannot run here, and say so when installed.', installedHost),
    settingBlock('Marketplace', 'Extensions come from Open VSX (open-vsx.org), the open registry. Each download is checked against the registry checksum and only the theme and snippet files are unpacked.', market),
  ];
}
