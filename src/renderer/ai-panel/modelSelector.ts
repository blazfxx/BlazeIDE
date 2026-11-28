// Model selector: the BlazeAPI account's own model list, searchable and
// grouped by maker, with live prices. The left button shows the account and
// opens it (balance, top up, sign out).

import type { ModelEntry, ModelKind } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { icon, iconChevronDown, iconAI, iconText, iconImage, iconVideo } from '../icons/index';
import { findModel } from './chatStore';
import { routingChoices, setRouting } from './routing';

const KINDS: Array<{ kind: ModelKind; label: string; svg: string }> = [
  { kind: 'chat', label: 'Text', svg: iconText },
  { kind: 'image', label: 'Image', svg: iconImage },
  { kind: 'video', label: 'Video', svg: iconVideo },
];
const KIND_ICON: Record<ModelKind, string> = { chat: iconText, image: iconImage, video: iconVideo };

function ctxLabel(n: number): string {
  if (!n) return '';
  return n >= 1_000_000 ? `${+(n / 1_000_000).toFixed(1)}M` : `${Math.round(n / 1000)}k`;
}

function priceLabel(m: ModelEntry): string {
  if (m.free || (!m.priceIn && !m.priceOut && m.kind === 'chat')) return '';
  if (m.kind !== 'chat') return `$${+m.priceIn.toPrecision(2)} each`;
  const f = (n: number): string => (n >= 1 ? n.toFixed(2) : n >= 0.1 ? n.toFixed(3) : n.toPrecision(2)).replace(/\.?0+$/, '');
  return `$${f(m.priceIn)} / $${f(m.priceOut)}`;
}

export function initModelSelector(host: HTMLElement): void {
  const providerBtn = el('button', 'provider-toggle');
  providerBtn.title = 'BlazeAPI account';
  const modelBtn = el('button', 'model-toggle');
  host.append(providerBtn, modelBtn);

  let dropdown: HTMLElement | null = null;

  function currentModelLabel(): string {
    const m = findModel(state.settings.selectedModel);
    return m?.displayName ?? state.settings.selectedModel;
  }

  function renderButtons(): void {
    providerBtn.innerHTML = `${icon(iconAI, 'icon-12')} BlazeAPI`;
    modelBtn.innerHTML = `${escapeHtml(currentModelLabel())} ${icon(iconChevronDown, 'icon-12')}`;
  }

  function closeDropdown(): void {
    dropdown?.remove();
    dropdown = null;
    document.removeEventListener('mousedown', onAway, true);
  }

  function onAway(e: MouseEvent): void {
    if (dropdown && !dropdown.contains(e.target as Node) && !modelBtn.contains(e.target as Node)) closeDropdown();
  }

  let kindFilter: ModelKind = 'chat';
  let loadError = '';
  bus.on('models-error', (m) => { loadError = m; });
  bus.on('models-changed', () => { loadError = ''; });

  function isSelected(m: ModelEntry): boolean {
    if (m.kind === 'image') return m.id === state.settings.imageModel;
    if (m.kind === 'video') return m.id === state.settings.videoModel;
    return m.id === state.settings.selectedModel;
  }

  function selectModel(m: ModelEntry): void {
    const patch = m.kind === 'image' ? { imageModel: m.id } : m.kind === 'video' ? { videoModel: m.id } : { selectedModel: m.id };
    Object.assign(state.settings, patch);
    void window.void.settingsSet(patch);
    if (m.kind === 'chat') closeDropdown();
    renderButtons();
    bus.emit('ai-model-selected', undefined);
  }

  function modelRow(m: ModelEntry): HTMLElement {
    const row = el('div', `dd-item${isSelected(m) ? ' selected' : ''}`);
    const price = priceLabel(m);
    row.title = `${m.id}${price ? ` · ${price}${m.kind === 'chat' ? ' per 1M tokens (in / out)' : ''}` : ''}${m.kind === 'video' ? ' · animates an image into a short clip' : ''}`;
    row.appendChild(el('span', 'dd-kind-icon', icon(KIND_ICON[m.kind], 'icon-12')));
    row.appendChild(el('span', 'dd-name', escapeHtml(m.displayName)));
    if (m.free) row.appendChild(el('span', 'badge badge-free', 'FREE'));
    if (m.vision && m.kind === 'chat') row.appendChild(el('span', 'badge', 'VISION'));
    if (m.tags.includes('needs-balance')) {
      row.appendChild(el('span', 'badge badge-balance', 'NEEDS BALANCE'));
      row.title += ' · add balance in your BlazeAPI dashboard to use it';
    }
    if (price) row.appendChild(el('span', 'dd-ctx', escapeHtml(price)));
    if (m.kind === 'chat') row.appendChild(el('span', 'dd-ctx', ctxLabel(m.contextWindow)));
    row.addEventListener('click', () => selectModel(m));
    return row;
  }

  function renderList(list: HTMLElement, query: string): void {
    list.innerHTML = '';
    const q = query.trim().toLowerCase();
    const matching = state.models.filter((m) => m.kind === kindFilter && (!q || m.displayName.toLowerCase().includes(q) || m.id.toLowerCase().includes(q) || m.provider.toLowerCase().includes(q)));
    if (kindFilter === 'image') {
      // "Automatic" lets the IDE pick (GPT Image first), the default.
      const auto = el('div', `dd-item${state.settings.imageModel ? '' : ' selected'}`);
      auto.appendChild(el('span', 'dd-kind-icon', icon(iconImage, 'icon-12')));
      auto.appendChild(el('span', 'dd-name', 'Automatic'));
      auto.addEventListener('click', () => { Object.assign(state.settings, { imageModel: '' }); void window.void.settingsSet({ imageModel: '' }); renderList(list, query); });
      if (!q) list.appendChild(auto);
      if (!state.settings.aiMediaGen) list.appendChild(el('div', 'dd-empty', 'Image generation is off. Turn it on in Settings > AI.'));
    }
    if (matching.length === 0) {
      if (state.models.length === 0 && loadError) {
        const box = el('div', 'dd-empty', escapeHtml(`Could not load models: ${loadError}`));
        const retry = el('button', 'prov-preset', 'Retry');
        retry.addEventListener('click', () => {
          list.innerHTML = '';
          list.appendChild(el('div', 'dd-empty', 'Loading models…'));
          void window.void.aiRefreshModels().then((m) => { state.models = m; loadError = ''; bus.emit('models-changed', m); renderList(list, query); }, (e) => { loadError = String(e); renderList(list, query); });
        });
        box.appendChild(retry);
        list.appendChild(box);
        return;
      }
      const none = state.models.length === 0 ? 'Loading models…' : kindFilter === 'video' ? 'No video models on BlazeAPI yet.' : 'No models match.';
      list.appendChild(el('div', 'dd-empty', none));
      return;
    }
    const free = matching.filter((m) => m.free);
    const paid = matching.filter((m) => !m.free);
    const custom = paid.filter((m) => m.tags.includes('custom'));
    const groups = new Map<string, ModelEntry[]>();
    if (free.length) groups.set('Free', free);
    for (const m of paid) {
      if (m.tags.includes('custom')) continue;
      const g = groups.get(m.provider) ?? [];
      g.push(m);
      groups.set(m.provider, g);
    }
    for (const m of custom) {
      const key = `${m.provider} (custom)`;
      const g = groups.get(key) ?? [];
      g.push(m);
      groups.set(key, g);
    }
    for (const [name, models] of groups) {
      list.appendChild(el('div', 'dd-section', escapeHtml(name.toUpperCase())));
      for (const m of models) list.appendChild(modelRow(m));
    }
  }

  function openDropdown(): void {
    if (dropdown) {
      closeDropdown();
      return;
    }
    dropdown = el('div', 'dropdown model-dropdown');
    const search = el('input', 'dd-search') as HTMLInputElement;
    search.placeholder = 'Search models';
    search.spellcheck = false;
    const list = el('div', 'dd-list');
    const kinds = el('div', 'dd-kinds');
    for (const k of KINDS) {
      const count = state.models.filter((m) => m.kind === k.kind).length;
      const chip = el('button', `dd-kind${k.kind === kindFilter ? ' active' : ''}`, `${icon(k.svg, 'icon-12')}<span>${k.label}</span>`);
      chip.title = count ? `${count} ${k.label.toLowerCase()} models` : `No ${k.label.toLowerCase()} models yet`;
      chip.addEventListener('click', () => {
        kindFilter = k.kind;
        kinds.querySelectorAll('.dd-kind').forEach((c) => c.classList.remove('active'));
        chip.classList.add('active');
        renderList(list, search.value);
      });
      kinds.appendChild(chip);
    }
    // Routing: Account / Normal / Direct (+ Cheapest for the owner).
    const routing = el('div', 'dd-routing');
    routing.appendChild(el('span', 'dd-routing-label', 'Routing'));
    for (const c of routingChoices()) {
      const b = el('button', `dd-route${(state.settings.aiRouting ?? '') === c.value ? ' active' : ''}${c.admin ? ' admin' : ''}`, escapeHtml(c.label));
      b.title = c.hint;
      b.addEventListener('click', () => {
        if ((state.settings.aiRouting ?? '') === c.value) return;
        routing.querySelectorAll('.dd-route').forEach((x) => x.classList.remove('active'));
        b.classList.add('active');
        list.innerHTML = '';
        list.appendChild(el('div', 'dd-empty', 'Loading models…'));
        void setRouting(c.value).then(() => renderList(list, search.value), (e) => { loadError = String(e); renderList(list, search.value); });
      });
      routing.appendChild(b);
    }
    dropdown.append(search, routing, kinds, list);
    renderList(list, '');
    search.addEventListener('input', () => renderList(list, search.value));
    search.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') closeDropdown();
      if (e.key === 'Enter') {
        const first = list.querySelector<HTMLElement>('.dd-item');
        first?.click();
      }
    });
    host.appendChild(dropdown);
    search.focus();
    document.addEventListener('mousedown', onAway, true);
  }

  providerBtn.addEventListener('click', () => bus.emit('open-account', undefined));
  modelBtn.addEventListener('click', openDropdown);

  bus.on('session-changed', renderButtons);
  bus.on('models-changed', renderButtons);
  renderButtons();
}
