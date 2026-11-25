// Sidebar > Extensions: search Open VSX, the installed list and the
// marketplace, VS Code style. Clicking a row opens the extension's page
// (details.ts) in an editor tab.

import type { InstalledExt, MarketItem } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { icon, iconChevronDown, iconChevronRight } from '../icons/index';
import { themeKey } from './apply';
import { compact, ensureLoaded, installExt, installedList, isBusy, onExtChange, summary, uninstallExt } from './store';

const CATEGORIES: Array<{ label: string; value: string }> = [
  { label: 'All', value: '' },
  { label: 'Themes', value: 'Themes' },
  { label: 'Snippets', value: 'Snippets' },
];

function avatar(name: string, url: string | null | undefined): HTMLElement {
  const a = el('div', 'ext-avatar', escapeHtml((name[0] ?? '?').toUpperCase()));
  if (url && (url.startsWith('data:image/') || url.startsWith('https://open-vsx.org/'))) {
    const img = document.createElement('img');
    img.alt = '';
    img.referrerPolicy = 'no-referrer';
    img.addEventListener('load', () => { a.textContent = ''; a.classList.add('has-icon'); a.appendChild(img); });
    img.src = url;
  }
  return a;
}

function splitId(id: string): { namespace: string; name: string } {
  const i = id.indexOf('.');
  return { namespace: id.slice(0, i), name: id.slice(i + 1) };
}

export function buildExtensionsView(host: HTMLElement): void {
  host.textContent = '';
  const top = el('div', 'xv-top');
  const search = el('input', 'xv-search') as HTMLInputElement;
  search.type = 'search';
  search.placeholder = 'Search extensions in Open VSX';
  search.spellcheck = false;
  const chips = el('div', 'xv-chips');
  top.append(search, chips);

  const scroll = el('div', 'xv-scroll');
  const instHead = el('button', 'xv-sec');
  const instBody = el('div', 'xv-list');
  const marketHead = el('button', 'xv-sec');
  const status = el('div', 'xv-status');
  const marketBody = el('div', 'xv-list');
  scroll.append(instHead, instBody, marketHead, status, marketBody);
  host.append(top, scroll);

  let category = '';
  let items: MarketItem[] = [];
  let total = 0;
  let seq = 0;
  let timer: number | undefined;
  const open = { installed: true, market: true };

  const chipEls = CATEGORIES.map((c) => {
    const b = el('button', 'xv-chip', escapeHtml(c.label));
    b.addEventListener('click', () => { category = c.value; markChips(); void run(false); });
    chips.appendChild(b);
    return b;
  });
  function markChips(): void {
    CATEGORIES.forEach((c, i) => chipEls[i].classList.toggle('active', c.value === category));
  }
  markChips();

  function section(head: HTMLElement, body: HTMLElement, label: string, count: number | null, key: 'installed' | 'market'): void {
    head.innerHTML = `${icon(open[key] ? iconChevronDown : iconChevronRight)}<span>${escapeHtml(label)}</span>${count === null ? '' : `<span class="xv-count">${count}</span>`}`;
    body.style.display = open[key] ? '' : 'none';
    head.onclick = () => { open[key] = !open[key]; renderAll(); };
  }

  function actionButton(id: string, namespace: string, name: string): HTMLElement {
    const have = installedList().some((e) => e.id === id);
    const busy = isBusy(id);
    const b = el('button', have ? 'xv-btn' : 'xv-btn primary', busy ? 'Installing…' : have ? 'Uninstall' : 'Install') as HTMLButtonElement;
    b.disabled = busy;
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      status.className = 'xv-status';
      const job = have ? uninstallExt(id) : installExt(namespace, name).then((ext) => {
        status.textContent = `Installed ${ext.displayName}. ${summary(ext)}.`;
      });
      job.catch((err) => { status.className = 'xv-status err'; status.textContent = String(err); renderAll(); });
    });
    return b;
  }

  function row(opts: {
    id: string; namespace: string; name: string; display: string; description: string; iconUrl: string | null | undefined;
    publisher: string; verified: boolean; downloads?: number; rating?: number | null; version: string; note?: string; warn?: boolean; item?: MarketItem;
  }): HTMLElement {
    const r = el('div', 'xv-row');
    r.tabIndex = 0;
    r.title = `${opts.display}\n${opts.description}`;
    r.dataset.id = opts.id;
    const text = el('div', 'xv-text');
    const line1 = el('div', 'xv-line1');
    line1.appendChild(el('span', 'xv-name', escapeHtml(opts.display)));
    const stats = el('span', 'xv-stats');
    if (opts.downloads !== undefined) stats.appendChild(el('span', '', `⤓ ${compact(opts.downloads)}`));
    if (opts.rating) stats.appendChild(el('span', '', `★ ${opts.rating.toFixed(1)}`));
    if (opts.downloads === undefined) stats.appendChild(el('span', '', `v${escapeHtml(opts.version)}`));
    line1.appendChild(stats);
    text.appendChild(line1);
    text.appendChild(el('div', `xv-desc${opts.warn ? ' warn' : ''}`, escapeHtml(opts.note ?? (opts.description || 'No description'))));
    const foot = el('div', 'xv-foot');
    foot.appendChild(el('span', 'xv-pub', `${opts.verified ? '<span class="xv-verified" title="Verified publisher">✓</span>' : ''}${escapeHtml(opts.publisher)}`));
    foot.appendChild(actionButton(opts.id, opts.namespace, opts.name));
    text.appendChild(foot);
    r.append(avatar(opts.display, opts.iconUrl), text);
    const go = (): void => bus.emit('open-extension', { namespace: opts.namespace, name: opts.name, item: opts.item });
    r.addEventListener('click', go);
    r.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); go(); } });
    return r;
  }

  function installedRow(e: InstalledExt): HTMLElement {
    const { namespace, name } = splitId(e.id);
    const using = e.themes.some((t) => themeKey(e.id, t.label) === state.settings.editorTheme);
    return row({
      id: e.id, namespace, name, display: e.displayName, description: e.description, iconUrl: e.icon,
      publisher: namespace, verified: false, version: e.version,
      note: using ? 'Colour theme in use' : e.themes.length + e.snippets.length === 0 ? summary(e) : undefined,
      warn: !using && e.themes.length + e.snippets.length === 0,
    });
  }

  function renderInstalled(): void {
    const list = installedList();
    const q = search.value.trim().toLowerCase();
    const shown = q ? list.filter((e) => `${e.displayName} ${e.id} ${e.description}`.toLowerCase().includes(q)) : list;
    section(instHead, instBody, 'Installed', shown.length, 'installed');
    instBody.textContent = '';
    if (list.length === 0) instBody.appendChild(el('div', 'xv-empty', 'Nothing installed yet. Colour themes and snippets from the marketplace below work in Blaze IDE.'));
    else if (shown.length === 0) instBody.appendChild(el('div', 'xv-empty', 'No installed extension matches.'));
    for (const e of shown) instBody.appendChild(installedRow(e));
  }

  function renderMarket(): void {
    section(marketHead, marketBody, search.value.trim() ? 'Marketplace results' : 'Popular in the marketplace', null, 'market');
    status.style.display = open.market && status.textContent ? '' : 'none';
    marketBody.textContent = '';
    for (const it of items) {
      marketBody.appendChild(row({
        id: it.id, namespace: it.namespace, name: it.name, display: it.displayName, description: it.description, iconUrl: it.iconUrl,
        publisher: it.namespace, verified: it.verified, downloads: it.downloads, rating: it.rating, version: it.version, item: it,
      }));
    }
    if (items.length > 0 && items.length < total) {
      const more = el('button', 'xv-more', 'Show more');
      more.addEventListener('click', () => void run(true));
      marketBody.appendChild(more);
    }
  }

  function renderAll(): void {
    renderInstalled();
    renderMarket();
  }

  async function run(more: boolean): Promise<void> {
    const mine = ++seq;
    if (!more) { items = []; total = 0; }
    status.className = 'xv-status';
    status.textContent = 'Searching…';
    renderMarket();
    try {
      const page = await window.void.extSearch(search.value, category || undefined, more ? items.length : 0);
      if (mine !== seq) return;
      items = more ? [...items, ...page.items] : page.items;
      total = page.total;
      status.textContent = items.length === 0 ? 'No extensions found.' : '';
    } catch (e) {
      if (mine !== seq) return;
      status.className = 'xv-status err';
      status.textContent = String(e);
    }
    renderMarket();
  }

  search.addEventListener('input', () => {
    renderInstalled();
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void run(false), 300);
  });

  onExtChange(renderAll);
  bus.on('settings-changed', () => renderInstalled());
  void ensureLoaded().then(renderAll);
  void run(false);
}
