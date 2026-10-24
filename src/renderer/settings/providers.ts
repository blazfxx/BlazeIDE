// Settings > Custom providers: add any OpenAI-compatible endpoint (OpenRouter,
// a local Ollama or LM Studio, your own gateway). The key is handed to Rust and
// kept in the OS keychain; this page only ever learns whether one is saved.

import type { CustomProvider } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { settingBlock } from './controls';

const PRESETS: Array<{ name: string; baseUrl: string; hint: string }> = [
  { name: 'OpenRouter', baseUrl: 'https://openrouter.ai/api/v1', hint: 'API key from openrouter.ai' },
  { name: 'OpenAI', baseUrl: 'https://api.openai.com/v1', hint: 'API key from platform.openai.com' },
  { name: 'Ollama (local)', baseUrl: 'http://localhost:11434/v1', hint: 'No key needed' },
  { name: 'LM Studio (local)', baseUrl: 'http://localhost:1234/v1', hint: 'No key needed' },
];

async function refreshModels(): Promise<void> {
  const models = await window.void.aiRefreshModels().catch(() => state.models);
  state.models = models;
  bus.emit('models-changed', models);
}

function field(label: string, control: HTMLElement): HTMLElement {
  const f = el('label', 'prov-field');
  f.appendChild(el('span', 'prov-label', escapeHtml(label)));
  f.appendChild(control);
  return f;
}

function input(type: string, placeholder: string, value = ''): HTMLInputElement {
  const i = el('input', 'prov-input') as HTMLInputElement;
  i.type = type;
  i.placeholder = placeholder;
  i.value = value;
  i.spellcheck = false;
  i.autocomplete = 'off';
  return i;
}

function buildForm(existing: CustomProvider | null, done: () => void): HTMLElement {
  const form = el('div', 'prov-form');
  const name = input('text', 'e.g. OpenRouter', existing?.name ?? '');
  const url = input('url', 'https://…/v1', existing?.baseUrl ?? '');
  const key = input('password', existing?.hasKey ? 'Saved in your keychain (type to replace)' : 'API key (optional for local servers)');
  const models = el('textarea', 'prov-input prov-models') as HTMLTextAreaElement;
  models.placeholder = 'One model id per line, or press “Fetch models”';
  models.value = existing?.models.join('\n') ?? '';
  models.rows = 5;
  models.spellcheck = false;
  const status = el('div', 'prov-status');

  if (!existing) {
    const presets = el('div', 'prov-presets');
    for (const p of PRESETS) {
      const b = el('button', 'prov-preset', escapeHtml(p.name));
      b.title = p.hint;
      b.addEventListener('click', () => { name.value = p.name; url.value = p.baseUrl; status.textContent = p.hint; });
      presets.appendChild(b);
    }
    form.appendChild(presets);
  }

  const fetchBtn = el('button', '', 'Fetch models');
  fetchBtn.addEventListener('click', () => {
    status.className = 'prov-status';
    status.textContent = 'Asking the endpoint…';
    window.void.providersFetchModels(url.value, key.value || undefined, existing?.id).then(
      (list) => {
        models.value = list.join('\n');
        status.textContent = `${list.length} model${list.length === 1 ? '' : 's'} found.`;
      },
      (e) => { status.className = 'prov-status err'; status.textContent = String(e); },
    );
  });

  const save = el('button', 'primary', existing ? 'Save changes' : 'Add provider');
  save.addEventListener('click', () => {
    status.className = 'prov-status';
    window.void
      .providersSave({
        id: existing?.id,
        name: name.value,
        baseUrl: url.value,
        models: models.value.split('\n'),
        apiKey: key.value ? key.value : undefined,
      })
      .then(async () => { await refreshModels(); done(); })
      .catch((e) => { status.className = 'prov-status err'; status.textContent = String(e); });
  });
  const cancel = el('button', '', 'Cancel');
  cancel.addEventListener('click', done);

  const actions = el('div', 'form-actions');
  actions.append(fetchBtn, save, cancel);
  form.append(field('Name', name), field('Base URL', url), field('API key', key), field('Models', models), actions, status);
  return form;
}

export function buildProvidersSection(): HTMLElement[] {
  const host = el('div', 'prov-host');

  async function render(editing: CustomProvider | 'new' | null = null): Promise<void> {
    const list = await window.void.providersList().catch(() => [] as CustomProvider[]);
    host.textContent = '';
    for (const p of list) {
      if (editing && editing !== 'new' && editing.id === p.id) continue;
      const row = el('div', 'prov-row');
      const text = el('div', 'prov-row-text');
      text.appendChild(el('div', 'prov-row-name', escapeHtml(p.name)));
      text.appendChild(el('div', 'prov-row-sub', escapeHtml(`${p.baseUrl} · ${p.models.length} model${p.models.length === 1 ? '' : 's'}${p.hasKey ? ' · key saved' : ''}`)));
      const edit = el('button', '', 'Edit');
      edit.addEventListener('click', () => void render(p));
      const del = el('button', 'btn-danger', 'Remove');
      del.addEventListener('click', () => {
        void window.void.providersDelete(p.id).then(refreshModels).then(() => render());
      });
      row.append(text, edit, del);
      host.appendChild(row);
    }
    if (editing) {
      host.appendChild(buildForm(editing === 'new' ? null : editing, () => void render()));
    } else {
      const add = el('button', 'primary', 'Add a provider');
      add.addEventListener('click', () => void render('new'));
      host.appendChild(add);
    }
  }
  void render();

  return [
    settingBlock(
      'Bring your own endpoint',
      'Use any OpenAI-compatible API next to BlazeAPI: OpenRouter, OpenAI, a local Ollama or LM Studio, or your own gateway. Their models appear in the model picker under their name. Keys are stored in your system keychain, never in the app files, and requests go straight to that provider (not through BlazeAPI).',
      host,
    ),
  ];
}
