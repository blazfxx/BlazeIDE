// Settings tab — Cursor-style: left nav rail + searchable sections of rows.

import { REASONING_LEVELS } from '../../shared/types';
import { BILLING_URL, MODELS_URL } from '../../shared/modelCatalog';
import { formatUsd } from '../account/index';
import { bus, el, escapeHtml, state } from '../state';
import { iconSearch } from '../icons/index';
import type { EditorArea } from '../editor/index';
import { AMBIENT_PRESETS, setAmbientVolume } from '../ambientAudio';
import { buildKeybindingsSection } from './keybindings';
import { buildExtensionsSection } from './extensions';
import { buildProvidersSection } from './providers';
import {
  apply,
  filterSections,
  numberInput,
  selectInput,
  settingBlock,
  settingRow,
  sliderInput,
  toggle,
} from './controls';

function buildEditorSection(): HTMLElement[] {
  const s = state.settings;
  return [
    settingRow('Font size', 'Editor text size in pixels.', numberInput(s.editorFontSize, 10, 20, (v) => void apply({ editorFontSize: v }))),
    settingRow('Minimap', 'Show a condensed overview of the file on the right edge.', toggle(s.editorMinimap, (v) => void apply({ editorMinimap: v }))),
    settingRow('Relative line numbers', 'Number lines by distance from the cursor.', toggle(s.editorRelativeLineNumbers, (v) => void apply({ editorRelativeLineNumbers: v }))),
    settingRow('Word wrap', 'Wrap long lines instead of scrolling horizontally.', toggle(s.editorWordWrap, (v) => void apply({ editorWordWrap: v }))),
    settingRow('Auto-save', 'Save changes automatically shortly after you stop typing. Off = save manually with Cmd/Ctrl+S.', toggle(s.editorAutosave, (v) => void apply({ editorAutosave: v }))),
  ];
}

function buildWorkbenchSection(): HTMLElement[] {
  const s = state.settings;
  return [
    settingRow('Sidebar width', 'Width of the file tree panel, in pixels.', numberInput(s.sidebarWidth, 140, 420, (v) => void apply({ sidebarWidth: v }))),
    settingRow('AI panel width', 'Width of the AI chat panel, in pixels.', numberInput(s.aiPanelWidth, 240, 560, (v) => void apply({ aiPanelWidth: v }))),
    settingRow('Terminal height', 'Height of the terminal panel, in pixels.', numberInput(s.terminalHeight, 80, 800, (v) => void apply({ terminalHeight: v }))),
    settingRow('Confirm before deleting', 'Ask for confirmation when deleting files or folders.', toggle(s.confirmDelete, (v) => void apply({ confirmDelete: v }))),
  ];
}

function buildTerminalSection(): HTMLElement[] {
  const s = state.settings;
  return [
    settingRow('Font size', 'Terminal text size in pixels.', numberInput(s.terminalFontSize, 10, 20, (v) => void apply({ terminalFontSize: v }))),
  ];
}

function buildAISection(): HTMLElement[] {
  const s = state.settings;
  const models = state.models.filter((m) => m.kind === 'chat').map((m) => ({ value: m.id, label: `${m.displayName} (${m.provider})` }));

  const reasoningOptions = REASONING_LEVELS.map((lvl) => ({
    value: lvl,
    label: lvl === 'off' ? 'Off' : lvl.charAt(0).toUpperCase() + lvl.slice(1),
  }));

  const browse = el('button', '', 'Browse all BlazeAPI models');
  browse.addEventListener('click', () => void window.void.openExternal(MODELS_URL));

  return [
    settingRow('Default model', 'Model used for new conversations. Every BlazeAPI model your account can use is listed.', selectInput(models, s.selectedModel, (v) => void apply({ selectedModel: v }))),
    settingRow('Reasoning effort', 'How hard models think before answering. Higher is slower and more thorough; Off disables thinking where supported.', selectInput(reasoningOptions, s.reasoningEffort, (v) => void apply({ reasoningEffort: v as typeof s.reasoningEffort }))),
    settingRow('Auto-approve agent actions', 'Agent will modify files and run commands without asking. Use with care.', toggle(s.aiAutoApprove, (v) => void apply({ aiAutoApprove: v }))),
    settingRow('Autocomplete', 'Ghost-text suggestions while you type; press Tab to accept. Uses a very cheap fast model by default (a fraction of a cent per hundred suggestions) and falls back to a free one.', toggle(s.aiAutocomplete, (v) => void apply({ aiAutocomplete: v }))),
    settingRow('Autocomplete model', 'Which model writes the suggestions. Automatic picks a cheap fast one.', selectInput([{ value: '', label: 'Automatic (cheap, fast)' }, ...models], s.aiAutocompleteModel, (v) => void apply({ aiAutocompleteModel: v }))),
    settingRow('Image generation', 'Off by default. When on, the AI agent can generate images as project assets (hero images, icons, logos), billed to your BlazeAPI balance per image. Pick the image model in the model picker.', toggle(s.aiMediaGen, (v) => void apply({ aiMediaGen: v }))),
    settingBlock('Models and prices', 'Prices are per 1M tokens and come straight from BlazeAPI.', browse),
  ];
}

function buildPrivacySection(): HTMLElement[] {
  const s = state.settings;
  return [
    settingRow(
      'Share anonymous usage',
      'Off by default. When on, Blaze IDE sends a random install id, the app version, your platform and event names (app opened, signed in, AI request with the BlazeAPI model id, updated). Never your files, prompts, paths or custom providers.',
      toggle(s.analytics, (v) => void apply({ analytics: v }))
    ),
  ];
}

function buildAccountSection(): HTMLElement[] {
  const rows: HTMLElement[] = [];
  if (!state.session) return rows;

  const info = el('div', 'account-line');
  info.appendChild(el('span', 'account-line-email', escapeHtml(state.session.userEmail || state.session.username)));
  const bal = el('span', 'account-line-balance', 'Balance: …');
  info.appendChild(bal);
  void window.void.authGetUsage().then((u) => { bal.textContent = u ? `Balance: ${formatUsd(u.balanceUsd)}` : ''; });
  rows.push(settingBlock('BlazeAPI account', '', info));

  const actions = el('div', 'form-actions');
  const top = el('button', 'primary', 'Top up balance');
  top.addEventListener('click', () => void window.void.openExternal(BILLING_URL));
  actions.appendChild(top);
  const out = el('button', 'btn-danger', 'Sign out');
  out.addEventListener('click', () => {
    void window.void.authLogout().then(() => bus.emit('session-changed', null));
  });
  actions.appendChild(out);
  rows.push(settingBlock('Billing', 'Blaze IDE is pay as you go: requests are billed to your BlazeAPI balance at the model\'s price.', actions));
  return rows;
}

function buildAppearanceSection(): HTMLElement[] {
  const s = state.settings;
  const rows: HTMLElement[] = [];

  rows.push(
    settingRow(
      'Theme',
      'Switch between the dark and light look.',
      selectInput(
        [{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }],
        s.theme,
        (v) => void apply({ theme: v as typeof s.theme })
      )
    )
  );

  // Custom background: pick an image file (stored inline as a data URL).
  const bgControl = el('div', 'bg-control');
  const pickBtn = el('button', '', s.backgroundImage ? 'Change image' : 'Choose image…');
  const clearBtn = el('button', '', 'Remove');
  clearBtn.style.display = s.backgroundImage ? '' : 'none';
  const fileInput = el('input') as HTMLInputElement;
  fileInput.type = 'file';
  fileInput.accept = 'image/*';
  fileInput.style.display = 'none';
  pickBtn.addEventListener('click', () => fileInput.click());
  fileInput.addEventListener('change', () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    if (file.size > 8 * 1024 * 1024) { alert('Please choose an image under 8 MB.'); return; }
    const reader = new FileReader();
    reader.onload = () => {
      void apply({ backgroundImage: String(reader.result), backgroundVideo: null });
      clearBtn.style.display = '';
      pickBtn.textContent = 'Change image';
    };
    reader.readAsDataURL(file);
  });
  clearBtn.addEventListener('click', () => {
    void apply({ backgroundImage: null });
    clearBtn.style.display = 'none';
    pickBtn.textContent = 'Choose image…';
  });
  bgControl.append(pickBtn, clearBtn, fileInput);
  rows.push(settingRow('Custom background', 'Show an image behind the workbench. Stored locally on this machine.', bgControl));

  // Video background: picked via the native dialog (path reference, not inlined)
  // and played as a muted loop. Mutually exclusive with a background image.
  const vidControl = el('div', 'bg-control');
  const vidPickBtn = el('button', '', s.backgroundVideo ? 'Change video' : 'Choose video…');
  const vidClearBtn = el('button', '', 'Remove');
  vidClearBtn.style.display = s.backgroundVideo ? '' : 'none';
  vidPickBtn.addEventListener('click', () => {
    void window.void.pickMedia('video').then((picked) => {
      if (!picked) return;
      // Picking a video replaces any image background.
      void apply({ backgroundVideo: picked, backgroundImage: null });
      vidClearBtn.style.display = '';
      vidPickBtn.textContent = 'Change video';
      adjustBtn.disabled = false;
    });
  });
  vidClearBtn.addEventListener('click', () => {
    void apply({ backgroundVideo: null });
    vidClearBtn.style.display = 'none';
    vidPickBtn.textContent = 'Choose video…';
    adjustBtn.disabled = !s.backgroundImage;
  });
  vidControl.append(vidPickBtn, vidClearBtn);
  rows.push(settingRow('Video background', 'Play a looping, muted video behind the workbench. Plays from the original file on this machine.', vidControl));

  // Fit mode: Fill crops to cover the window (may upscale/blur a small image);
  // Fit shows the whole image without forcing it past its natural size.
  rows.push(
    settingRow(
      'Image fit',
      'Fill covers the whole window (may crop and zoom in). Fit shows the entire image without over-zooming — sharper for smaller images.',
      selectInput(
        [{ value: 'cover', label: 'Fill (crop)' }, { value: 'contain', label: 'Fit (whole image)' }],
        s.backgroundFit || 'cover',
        (v) => void apply({ backgroundFit: v as 'cover' | 'contain' })
      )
    )
  );

  // Drag-to-adjust: opens a full-window mode to move/zoom/rotate the wallpaper.
  const adjustBtn = el('button', '', 'Adjust position, zoom & rotation');
  adjustBtn.disabled = !s.backgroundImage && !s.backgroundVideo;
  adjustBtn.addEventListener('click', () => {
    void import('../backgroundAdjust').then((m) => m.openBackgroundAdjust());
  });
  rows.push(settingRow('Position, zoom & rotation', 'Drag to move, scroll to zoom, Shift+scroll to rotate.', adjustBtn));

  rows.push(
    settingRow(
      'Background blur',
      'Blur the custom background (0 = sharp).',
      numberInput(s.backgroundBlur, 0, 40, (v) => void apply({ backgroundBlur: v }))
    )
  );
  rows.push(
    settingRow(
      'Sharpen / enhance',
      'Crisper scaling and a contrast/colour boost so the background looks clearer.',
      toggle(s.backgroundSharpen, (v) => void apply({ backgroundSharpen: v }))
    )
  );
  rows.push(
    settingRow(
      'Enhance strength',
      'How strong the sharpen/enhance effect is, 0–100%.',
      numberInput(s.backgroundSharpenAmount, 0, 100, (v) => void apply({ backgroundSharpenAmount: v }))
    )
  );
  rows.push(
    settingRow(
      'Auto-contrast text',
      'Adjust text colour to stay readable against your background (light text on dark images, dark on light).',
      toggle(s.backgroundAutoContrast, (v) => void apply({ backgroundAutoContrast: v }))
    )
  );
  rows.push(
    settingRow(
      'Background strength',
      'How visible the background is, 0–100%.',
      numberInput(Math.round(s.backgroundOpacity * 100), 0, 100, (v) => void apply({ backgroundOpacity: v / 100 }))
    )
  );

  return rows;
}

function buildSoundsSection(): HTMLElement[] {
  const s = state.settings;
  const rows: HTMLElement[] = [];

  rows.push(
    settingRow(
      'Ambient sound',
      'Play a looping background sound while you work. Loops seamlessly; resumes on launch.',
      toggle(s.ambientEnabled, (v) => void apply({ ambientEnabled: v }))
    )
  );

  const soundOptions = [
    ...AMBIENT_PRESETS.map((p) => ({ value: p.id, label: p.label })),
    { value: 'custom', label: 'Custom (uploaded)' },
  ];
  rows.push(
    settingRow(
      'Sound',
      'Choose a built-in ambient track, or upload your own below.',
      selectInput(soundOptions, s.ambientSource, (v) => void apply({ ambientSource: v }))
    )
  );

  // Upload your own audio (native picker → local path, played via blazemedia:).
  const upControl = el('div', 'bg-control');
  const fileName = s.ambientCustomPath ? s.ambientCustomPath.split('/').pop() ?? '' : '';
  const pickBtn = el('button', '', fileName ? 'Change audio' : 'Choose audio…');
  const nameLabel = el('span', 'set-inline-note', escapeHtml(fileName));
  const clearBtn = el('button', '', 'Remove');
  clearBtn.style.display = s.ambientCustomPath ? '' : 'none';
  pickBtn.addEventListener('click', () => {
    void window.void.pickMedia('audio').then((picked) => {
      if (!picked) return;
      void apply({ ambientCustomPath: picked, ambientSource: 'custom', ambientEnabled: true });
      nameLabel.textContent = picked.split('/').pop() ?? '';
      pickBtn.textContent = 'Change audio';
      clearBtn.style.display = '';
    });
  });
  clearBtn.addEventListener('click', () => {
    const fallback = AMBIENT_PRESETS[0]?.id ?? 'babft';
    void apply({ ambientCustomPath: null, ambientSource: fallback });
    nameLabel.textContent = '';
    pickBtn.textContent = 'Choose audio…';
    clearBtn.style.display = 'none';
  });
  upControl.append(pickBtn, nameLabel, clearBtn);
  rows.push(settingRow('Upload your own', 'Use a local audio file (mp3, ogg, wav, m4a, flac). Played from your machine.', upControl));

  rows.push(
    settingRow(
      'Volume',
      'Ambient sound volume.',
      sliderInput(
        Math.round(s.ambientVolume * 100),
        0,
        100,
        (v) => setAmbientVolume(v / 100), // live while dragging (no IPC)
        (v) => `${v}%`,
        (v) => void apply({ ambientVolume: v / 100 }) // persist on release
      )
    )
  );

  return rows;
}

const SECTIONS: Array<{ id: string; label: string; build: () => HTMLElement[] }> = [
  { id: 'sec-editor', label: 'Editor', build: buildEditorSection },
  { id: 'sec-workbench', label: 'Workbench', build: buildWorkbenchSection },
  { id: 'sec-terminal', label: 'Terminal', build: buildTerminalSection },
  { id: 'sec-ai', label: 'AI', build: buildAISection },
  { id: 'sec-extensions', label: 'Extensions', build: buildExtensionsSection },
  { id: 'sec-providers', label: 'Custom providers', build: buildProvidersSection },
  { id: 'sec-account', label: 'Account', build: buildAccountSection },
  { id: 'sec-privacy', label: 'Privacy', build: buildPrivacySection },
  { id: 'sec-keybindings', label: 'Keybindings', build: buildKeybindingsSection },
  { id: 'sec-appearance', label: 'Appearance', build: buildAppearanceSection },
  { id: 'sec-sounds', label: 'Sounds', build: buildSoundsSection },
];

export function openSettingsTab(editor: EditorArea): void {
  editor.openCustomTab('settings', 'Settings', (host) => {
    const page = el('div', 'set-page');
    const nav = el('div', 'set-nav');
    const content = el('div', 'set-content');

    // Search
    const searchWrap = el('div', 'set-search');
    searchWrap.appendChild(el('span', 'icon icon-12 set-search-icon', iconSearch));
    const search = el('input', 'set-search-input');
    search.placeholder = 'Search settings';
    search.spellcheck = false;
    searchWrap.appendChild(search);

    const navList = el('div', 'set-nav-list');
    nav.append(searchWrap, navList);

    // Sections render as switchable pages: exactly one visible at a time.
    const navItems = new Map<string, HTMLElement>();
    const sectionEls = new Map<string, HTMLElement>();
    let activeId: string | null = null;
    let searching = false;

    function showSection(id: string): void {
      activeId = id;
      for (const [sid, sectionEl] of sectionEls) {
        sectionEl.style.display = sid === id ? '' : 'none';
      }
      for (const [sid, item] of navItems) item.classList.toggle('active', sid === id);
      content.scrollTop = 0;
    }

    for (const def of SECTIONS) {
      const rows = def.build();
      if (rows.length === 0) continue;
      const sectionEl = el('div', 'set-section');
      sectionEl.id = def.id;
      sectionEl.appendChild(el('h2', 'set-section-title', escapeHtml(def.label)));
      for (const r of rows) sectionEl.appendChild(r);
      content.appendChild(sectionEl);
      sectionEls.set(def.id, sectionEl);

      const item = el('div', 'set-nav-item', escapeHtml(def.label));
      item.dataset.target = def.id;
      item.addEventListener('click', () => {
        if (searching) {
          search.value = '';
          searching = false;
          filterSections(content, navList, '');
        }
        showSection(def.id);
      });
      navList.appendChild(item);
      navItems.set(def.id, item);
    }

    // Search spans every section: show all matching rows across pages while
    // a query is active; restore the single-page view when cleared.
    search.addEventListener('input', () => {
      const q = search.value.trim();
      if (q === '') {
        searching = false;
        filterSections(content, navList, '');
        if (activeId) showSection(activeId);
        return;
      }
      searching = true;
      for (const sectionEl of sectionEls.values()) sectionEl.style.display = '';
      for (const item of navItems.values()) item.classList.remove('active');
      filterSections(content, navList, q);
    });

    const firstId = sectionEls.keys().next().value;
    if (typeof firstId === 'string') showSection(firstId);

    page.append(nav, content);
    host.appendChild(page);
  });
}
