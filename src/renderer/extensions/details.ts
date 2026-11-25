// An extension's page in an editor tab, like VS Code's: icon, name,
// publisher profile, stats, Install / Uninstall / Use theme, and tabs for the
// README, what runs in Blaze IDE, and the changelog. A side column has the
// facts (version, size, licence, categories, links) and the publisher card.

import type { EditorArea } from '../editor/index';
import type { ExtDetails, MarketItem } from '../../shared/types';
import { el, escapeHtml, state } from '../state';
import { themeKey } from './apply';
import { renderMarkdown } from './markdown';
import { compact, ensureLoaded, findInstalled, installExt, isBusy, onExtChange, summary, uninstallExt, useTheme } from './store';

const cache = new Map<string, Promise<ExtDetails>>();

function load(namespace: string, name: string, fresh = false): Promise<ExtDetails> {
  const id = `${namespace}.${name}`;
  let p = fresh ? undefined : cache.get(id);
  if (!p) {
    p = window.void.extDetails(namespace, name);
    cache.set(id, p);
    p.catch(() => cache.delete(id));
  }
  return p;
}

function when(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

function size(n: number): string {
  return n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : n ? `${n} B` : '';
}

function stars(r: number | null): string {
  if (r == null) return '';
  const full = Math.round(r);
  return '★★★★★'.slice(0, full) + '☆☆☆☆☆'.slice(0, 5 - full);
}

function link(label: string, url: string): HTMLElement | null {
  if (!/^https?:\/\//i.test(url)) return null;
  const a = el('a', 'xd-link', escapeHtml(label)) as HTMLAnchorElement;
  a.href = url;
  a.title = url;
  a.addEventListener('click', (e) => { e.preventDefault(); void window.void.openExternal(url); });
  return a;
}

function iconBox(name: string, url: string | null | undefined, color: string): HTMLElement {
  const box = el('div', 'xd-icon', escapeHtml((name[0] ?? '?').toUpperCase()));
  if (/^#[0-9a-f]{3,8}$/i.test(color)) box.style.setProperty('--xd-tint', color);
  if (url && (url.startsWith('data:image/') || url.startsWith('https://open-vsx.org/'))) {
    const img = document.createElement('img');
    img.alt = '';
    img.referrerPolicy = 'no-referrer';
    img.addEventListener('load', () => { box.textContent = ''; box.classList.add('has-icon'); box.appendChild(img); });
    img.src = url;
  }
  return box;
}

export function openExtensionDetails(editor: EditorArea, ref: { namespace: string; name: string; item?: MarketItem }): void {
  const id = `${ref.namespace}.${ref.name}`;
  const title = ref.item?.displayName ?? findInstalled(id)?.displayName ?? ref.name;
  editor.openCustomTab(`ext:${id}`, `Extension: ${title}`, (host) => render(host, ref));
}

function render(host: HTMLElement, ref: { namespace: string; name: string; item?: MarketItem }): void {
  const id = `${ref.namespace}.${ref.name}`;
  host.classList.add('xd-host');
  const page = el('div', 'xd');
  host.appendChild(page);

  // Header: drawn at once from what the list already knew, filled in when the
  // details arrive.
  const head = el('div', 'xd-head');
  const it = ref.item;
  const inst = findInstalled(id);
  const iconEl = iconBox(it?.displayName ?? inst?.displayName ?? ref.name, it?.iconUrl ?? inst?.icon, '');
  const info = el('div', 'xd-info');
  const nameEl = el('h1', 'xd-name', escapeHtml(it?.displayName ?? inst?.displayName ?? ref.name));
  const idEl = el('span', 'xd-id', escapeHtml(id));
  const byline = el('div', 'xd-byline');
  const desc = el('div', 'xd-desc', escapeHtml(it?.description ?? inst?.description ?? ''));
  const actions = el('div', 'xd-actions');
  const note = el('div', 'xd-note');
  info.append(el('div', 'xd-title-row'), byline, desc, actions, note);
  info.firstElementChild!.append(nameEl, idEl);
  head.append(iconEl, info);

  const tabsBar = el('div', 'xd-tabs');
  const body = el('div', 'xd-body');
  const main = el('div', 'xd-main');
  const side = el('aside', 'xd-side');
  body.append(main, side);
  page.append(head, tabsBar, body);
  main.appendChild(el('div', 'xd-loading', 'Loading the extension page from Open VSX…'));

  let d: ExtDetails | null = null;
  let tab: 'details' | 'features' | 'changelog' = 'details';
  const openLink = (u: string): void => { void window.void.openExternal(u); };

  function renderByline(): void {
    byline.textContent = '';
    const pubName = d?.publisher.name ?? ref.namespace;
    const pub = el('span', 'xd-pub');
    if (d?.publisher.avatar) {
      const img = document.createElement('img');
      img.src = d.publisher.avatar;
      img.alt = '';
      img.className = 'xd-pub-avatar';
      pub.appendChild(img);
    }
    pub.appendChild(el('span', '', escapeHtml(pubName)));
    if (d?.verified ?? it?.verified) pub.appendChild(el('span', 'xd-verified', '✓ Verified'));
    byline.appendChild(pub);
    const dl = d?.downloads ?? it?.downloads;
    if (dl !== undefined) byline.appendChild(el('span', 'xd-stat', `⤓ ${compact(dl)} installs`));
    const r = d?.rating ?? it?.rating ?? null;
    if (r != null) byline.appendChild(el('span', 'xd-stat xd-stars', `${stars(r)} <span>${r.toFixed(1)}${d?.reviewCount ? ` (${d.reviewCount})` : ''}</span>`));
    const v = d?.version ?? it?.version ?? inst?.version;
    if (v) byline.appendChild(el('span', 'xd-stat', `v${escapeHtml(v)}`));
  }

  function renderActions(): void {
    actions.textContent = '';
    const mine = findInstalled(id);
    const busy = isBusy(id);
    if (mine) {
      for (const t of mine.themes) {
        const key = themeKey(id, t.label);
        const using = state.settings.editorTheme === key;
        const b = el('button', using ? 'xd-btn' : 'xd-btn primary', using ? `✓ ${escapeHtml(t.label)} in use` : `Use ${escapeHtml(t.label)}`) as HTMLButtonElement;
        b.disabled = using;
        b.addEventListener('click', () => void useTheme(id, t.label).then(renderActions));
        actions.appendChild(b);
      }
      const un = el('button', 'xd-btn', 'Uninstall');
      un.addEventListener('click', () => {
        note.textContent = '';
        void uninstallExt(id).catch((e) => { note.className = 'xd-note err'; note.textContent = String(e); });
      });
      actions.appendChild(un);
      note.className = 'xd-note';
      note.textContent = `Installed v${mine.version}. ${summary(mine)}${mine.unsupported.length ? `. Not used here: ${mine.unsupported.join(', ')}` : ''}.`;
    } else {
      const b = el('button', 'xd-btn primary', busy ? 'Installing…' : 'Install') as HTMLButtonElement;
      b.disabled = busy;
      b.addEventListener('click', () => {
        note.className = 'xd-note';
        note.textContent = '';
        void installExt(ref.namespace, ref.name).catch((e) => { note.className = 'xd-note err'; note.textContent = String(e); });
      });
      actions.appendChild(b);
      if (d) {
        const runs = d.themes.length + d.snippets.length;
        note.className = runs ? 'xd-note' : 'xd-note warn';
        note.textContent = runs
          ? `Works in Blaze IDE: ${[d.themes.length ? `${d.themes.length} colour theme${d.themes.length === 1 ? '' : 's'}` : '', d.snippets.length ? `snippets for ${d.snippets.join(', ')}` : ''].filter(Boolean).join(' and ')}.`
          : 'Nothing in this extension can run in Blaze IDE (it needs the VS Code extension host).';
      }
    }
    const site = d && (link('Marketplace page', `https://open-vsx.org/extension/${ref.namespace}/${ref.name}`));
    if (site) { site.classList.add('xd-btn', 'ghost'); actions.appendChild(site); }
  }

  function renderTabs(): void {
    tabsBar.textContent = '';
    const defs: Array<[typeof tab, string, boolean]> = [
      ['details', 'Details', true],
      ['features', 'Features', true],
      ['changelog', 'Changelog', !!d?.changelog],
    ];
    for (const [k, label, show] of defs) {
      if (!show) continue;
      const b = el('button', `xd-tab${tab === k ? ' active' : ''}`, label);
      b.addEventListener('click', () => { tab = k; renderTabs(); renderMain(); });
      tabsBar.appendChild(b);
    }
  }

  function renderMain(): void {
    main.textContent = '';
    if (!d) return;
    if (tab === 'details') {
      if (d.readme.trim()) main.appendChild(renderMarkdown(d.readme, { images: d.images, onLink: openLink }));
      else main.appendChild(el('div', 'xd-empty', 'This extension has no README.'));
    } else if (tab === 'changelog') {
      main.appendChild(renderMarkdown(d.changelog, { images: d.images, onLink: openLink }));
    } else {
      const f = el('div', 'xd-features');
      const card = (title: string, rows: string[], kind: 'ok' | 'no'): void => {
        const c = el('div', `xd-feat ${kind}`);
        c.appendChild(el('h3', '', escapeHtml(title)));
        if (!rows.length) c.appendChild(el('div', 'xd-empty', 'None'));
        const ul = el('ul', '');
        for (const r of rows) ul.appendChild(el('li', '', escapeHtml(r)));
        if (rows.length) c.appendChild(ul);
        f.appendChild(c);
      };
      card('Colour themes', d.themes, 'ok');
      card('Snippets', d.snippets.map((l) => `Snippets for ${l}`), 'ok');
      card('Not available in Blaze IDE', d.unsupported, 'no');
      f.appendChild(el('p', 'xd-feat-note', 'Blaze IDE runs the declarative parts of VS Code extensions: colour themes and snippets. Parts that need the extension’s own code (commands, language servers, debuggers, icon themes) are not run.'));
      main.appendChild(f);
    }
  }

  function renderSide(): void {
    side.textContent = '';
    if (!d) return;
    // Publisher profile.
    const p = el('div', 'xd-card xd-publisher');
    p.appendChild(el('h3', '', 'Publisher'));
    const who = el('div', 'xd-who');
    const av = el('div', 'xd-who-avatar', escapeHtml((d.publisher.name[0] ?? '?').toUpperCase()));
    if (d.publisher.avatar) {
      const img = document.createElement('img');
      img.src = d.publisher.avatar;
      img.alt = '';
      av.textContent = '';
      av.appendChild(img);
    }
    const wt = el('div', 'xd-who-text');
    wt.appendChild(el('div', 'xd-who-name', `${escapeHtml(d.publisher.name)}${d.publisher.verified ? ' <span class="xd-verified" title="Verified publisher">✓</span>' : ''}`));
    const sub = [d.publisher.fullName && d.publisher.fullName !== d.publisher.name ? `Published by ${d.publisher.fullName}` : '', d.publisher.login ? `@${d.publisher.login}` : ''].filter(Boolean).join(' · ');
    if (sub) wt.appendChild(el('div', 'xd-who-sub', escapeHtml(sub)));
    if (d.publisher.extensions) wt.appendChild(el('div', 'xd-who-sub', `${d.publisher.extensions} extension${d.publisher.extensions === 1 ? '' : 's'} on Open VSX`));
    who.append(av, wt);
    p.appendChild(who);
    const plinks = el('div', 'xd-links');
    for (const l of [link('Profile', d.publisher.homepage), link('All extensions', `https://open-vsx.org/namespace/${d.publisher.namespace}`)]) if (l) plinks.appendChild(l);
    p.appendChild(plinks);
    side.appendChild(p);

    // Facts.
    const info = el('div', 'xd-card');
    info.appendChild(el('h3', '', 'Installation'));
    const dl = el('dl', 'xd-facts');
    const fact = (k: string, v: string): void => { if (v) { dl.appendChild(el('dt', '', escapeHtml(k))); dl.appendChild(el('dd', '', escapeHtml(v))); } };
    fact('Identifier', d.id);
    fact('Version', d.version);
    fact('Updated', when(d.updated));
    fact('Size', size(d.size));
    fact('Versions', d.versions ? String(d.versions) : '');
    fact('Licence', d.license);
    info.appendChild(dl);
    side.appendChild(info);

    if (d.categories.length || d.tags.length) {
      const c = el('div', 'xd-card');
      c.appendChild(el('h3', '', 'Categories'));
      const chips = el('div', 'xd-chips');
      for (const x of d.categories) chips.appendChild(el('span', 'xd-chip', escapeHtml(x)));
      for (const x of d.tags.slice(0, 12)) chips.appendChild(el('span', 'xd-chip tag', escapeHtml(x)));
      c.appendChild(chips);
      side.appendChild(c);
    }

    const res = el('div', 'xd-card');
    res.appendChild(el('h3', '', 'Resources'));
    const links = el('div', 'xd-links col');
    for (const l of [
      link('Marketplace', `https://open-vsx.org/extension/${d.namespace}/${d.name}`),
      link('Homepage', d.homepage),
      link('Repository', d.repository),
      link('Issues', d.bugs),
    ]) if (l) links.appendChild(l);
    res.appendChild(links);
    side.appendChild(res);
  }

  function renderAll(): void {
    renderByline();
    renderActions();
    renderTabs();
    renderMain();
    renderSide();
  }

  renderByline();
  renderActions();
  renderTabs();
  const off = onExtChange(() => {
    if (!host.contains(page)) { off(); return; } // the tab was re-rendered
    renderActions();
  });

  void ensureLoaded().then(() => renderActions());
  load(ref.namespace, ref.name).then(
    (det) => {
      d = det;
      if (det.iconUrl || det.galleryColor) iconEl.replaceWith(iconBox(det.displayName, det.iconUrl ?? it?.iconUrl ?? inst?.icon, det.galleryColor));
      nameEl.textContent = det.displayName;
      desc.textContent = det.description;
      renderAll();
    },
    (e) => {
      main.textContent = '';
      const err = el('div', 'xd-error');
      err.appendChild(el('div', '', escapeHtml(`Could not load this extension’s page: ${String(e)}`)));
      const retry = el('button', 'xd-btn', 'Try again');
      retry.addEventListener('click', () => { host.textContent = ''; render(host, ref); });
      err.appendChild(retry);
      main.appendChild(err);
    },
  );
}
