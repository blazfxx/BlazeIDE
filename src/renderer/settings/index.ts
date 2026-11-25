// Settings tab — Cursor-style: left nav rail + searchable sections of rows.

import { REASONING_LEVELS } from '../../shared/types';
import { BILLING_URL, MODELS_URL } from '../../shared/modelCatalog';
import { formatUsd } from '../account/index';
import { bus, el, escapeHtml, state } from '../state';
import { iconSearch } from '../icons/index';
import type { EditorArea } from '../editor/index';
import { AMBIENT_PRESETS, setAmbientVolume } from '../ambientAudio';
import { buildKeybindingsSection } from './keybindings';
import { buildProvidersSection } from './providers';
import { buildCliSection } from './cli';
import { buildMobileSection } from './mobile';
import { themeKey } from '../extensions/apply';
import type { InstalledExt } from '../../shared/types';
import { currentWork } from '../presence';
import { routingChoices, setRouting } from '../ai-panel/routing';
import type { AIRouting } from '../../shared/types';
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

function buildUpdatesSection(): HTMLElement[] {
  const s = state.settings;
  const status = el('span', 'update-status', 'Not checked yet');
  const checkBtn = el('button', '', 'Check for updates') as HTMLButtonElement;
  const updateBtn = el('button', 'primary', 'Update now') as HTMLButtonElement;
  updateBtn.disabled = true;
  let latest = '';
  const check = async (): Promise<void> => {
    checkBtn.disabled = true;
    status.textContent = 'Checking…';
    try {
      const v = await window.void.checkVersion();
      latest = v.latest;
      const newer = !!v.current && v.latest !== v.current && v.latest.localeCompare(v.current, undefined, { numeric: true }) > 0;
      status.textContent = newer ? `Blaze IDE ${v.latest} is available (you have ${v.current}).` : `You're up to date (${v.current ?? 'unknown version'}).`;
      updateBtn.disabled = !newer;
    } catch (e) {
      status.textContent = `Could not check: ${String(e)}`;
    } finally {
      checkBtn.disabled = false;
    }
  };
  checkBtn.addEventListener('click', () => void check());
  updateBtn.addEventListener('click', () => {
    updateBtn.disabled = true;
    checkBtn.disabled = true;
    status.textContent = `Downloading ${latest}…`;
    const off = window.void.onUpdateProgress((done, total) => {
      status.textContent = total ? `Downloading ${latest}… ${Math.min(100, Math.round((done / total) * 100))}%` : `Downloading ${latest}…`;
    });
    // Downloads, checks the signature, installs and restarts the app.
    window.void.installUpdate().then(() => off(), (e) => {
      off();
      status.textContent = `Update failed: ${String(e)}`;
      updateBtn.disabled = false;
      checkBtn.disabled = false;
    });
  });
  const actions = el('div', 'update-actions');
  actions.append(checkBtn, updateBtn, status);
  void check();
  return [
    settingRow('Automatic updates', 'Download new versions in the background (signature-checked) and install them when you quit the app (or right away with Restart now). Off = nothing is downloaded until you press Update now.', toggle(s.autoUpdate !== false, (v) => void apply({ autoUpdate: v }))),
    settingBlock('Check for updates', 'Look for a newer Blaze IDE now, and install it.', actions),
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

  const imageModels = state.models
    .filter((m) => m.kind === 'image' && !m.id.includes('edit'))
    .map((m) => ({ value: m.id, label: `${m.displayName}${m.priceIn ? ` · $${+m.priceIn.toPrecision(2)} each` : ''}` }));
  const supplierSelect = el('select', 'setting-select') as HTMLSelectElement;
  supplierSelect.addEventListener('change', () => void apply({ imageSupplier: supplierSelect.value }));
  function fillSuppliers(model: string): void {
    supplierSelect.innerHTML = '';
    const auto = el('option', '', 'Automatic') as HTMLOptionElement;
    auto.value = '';
    supplierSelect.appendChild(auto);
    supplierSelect.value = '';
    supplierSelect.disabled = !model;
    if (!model) return;
    void window.void.aiImageSuppliers(model).then((list) => {
      for (const sup of list) {
        const o = el('option', '', escapeHtml(`${sup.id}${sup.status && sup.status !== 'healthy' ? ` (${sup.status})` : ''}`)) as HTMLOptionElement;
        o.value = sup.id;
        supplierSelect.appendChild(o);
      }
      supplierSelect.value = list.some((x) => x.id === state.settings.imageSupplier) ? state.settings.imageSupplier : '';
    }, () => { /* leave Automatic only */ });
  }
  fillSuppliers(s.imageModel);

  const browse = el('button', '', 'Browse all BlazeAPI models');
  browse.addEventListener('click', () => void window.void.openExternal(MODELS_URL));

  return [
    settingRow('Default model', 'Model used for new conversations. Every BlazeAPI model your account can use is listed.', selectInput(models, s.selectedModel, (v) => void apply({ selectedModel: v }))),
    settingRow('Reasoning effort', 'How hard models think before answering. Higher is slower and more thorough; Off disables thinking where supported.', selectInput(reasoningOptions, s.reasoningEffort, (v) => void apply({ reasoningEffort: v as typeof s.reasoningEffort }))),
    settingRow('Auto-approve agent actions', 'Agent will modify files and run commands without asking. Use with care.', toggle(s.aiAutoApprove, (v) => void apply({ aiAutoApprove: v }))),
    settingRow('Autocomplete', 'Ghost-text suggestions while you type; press Tab to accept. Uses a very cheap fast model by default (a fraction of a cent per hundred suggestions) and falls back to a free one.', toggle(s.aiAutocomplete, (v) => void apply({ aiAutocomplete: v }))),
    settingRow('Autocomplete model', 'Which model writes the suggestions. Automatic picks a cheap fast one.', selectInput([{ value: '', label: 'Automatic (cheap, fast)' }, ...models], s.aiAutocompleteModel, (v) => void apply({ aiAutocompleteModel: v }))),
    settingRow('Routing', 'Which BlazeAPI routing chat requests use. Normal: the most models and the lowest prices. Direct: model makers and trusted resellers only. Account: whatever your BlazeAPI account is set to.', selectInput(routingChoices().map((c) => ({ value: c.value, label: c.admin ? `${c.label} (owner only)` : c.label })), s.aiRouting ?? '', (v) => void setRouting(v as AIRouting).then(() => apply({})))),
    settingRow('Image generation', 'Off by default. When on, the AI agent can generate images as project assets (hero images, icons, logos), billed to your BlazeAPI balance per image.', toggle(s.aiMediaGen, (v) => void apply({ aiMediaGen: v }))),
    settingRow('Image model', 'Model the agent uses for images. Automatic tries the models whose suppliers are delivering first, and if a model fails the next one is tried, so one model being out of stock never stops the agent.', selectInput([{ value: '', label: 'Automatic' }, ...imageModels], s.imageModel, (v) => void apply({ imageModel: v, imageSupplier: '' }).then(() => fillSuppliers(v)))),
    settingRow('Video generation', 'Off by default. When on, the agent can animate an image into a short video clip (hero or background videos). Each clip costs about $0.20-0.50 of your BlazeAPI balance.', toggle(s.aiVideoGen, (v) => void apply({ aiVideoGen: v }))),
    settingRow('Video model', 'Model for video clips. Automatic starts with fast, cheap ones and tries the next if one fails.', selectInput([{ value: '', label: 'Automatic' }, ...state.models.filter((m) => m.kind === 'video').map((m) => ({ value: m.id, label: `${m.displayName}${m.priceIn ? ` · $${+m.priceIn.toPrecision(2)} each` : ''}` }))], s.videoModel, (v) => void apply({ videoModel: v }))),
    settingRow('Image supplier', 'Which BlazeAPI supplier serves the chosen image model. Automatic lets BlazeAPI pick. Only applies when a specific image model is chosen.', supplierSelect),
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
    settingRow(
      'Discord status',
      'Shows "Working on <file> - BlazeCLI - blazeapi.org" on your Discord profile while Blaze IDE is open (Discord desktop app only). Your Discord friends can see the file and project names.',
      toggle(s.discordStatus, (v) => void apply({ discordStatus: v }).then(() => window.void.presenceSet(currentWork())))
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

  // Editor colour theme from an installed extension (Extensions sidebar).
  const themeHost = el('div', '');
  const renderThemes = (list: InstalledExt[]): void => {
    themeHost.textContent = '';
    const options = [{ value: '', label: 'Blaze default' }];
    for (const e of list) for (const t of e.themes) options.push({ value: themeKey(e.id, t.label), label: `${t.label} (${e.displayName})` });
    const current = options.some((o) => o.value === state.settings.editorTheme) ? state.settings.editorTheme : '';
    themeHost.appendChild(selectInput(options, current, (v) => void apply({ editorTheme: v })));
  };
  renderThemes([]);
  void window.void.extList().then(renderThemes, () => undefined);
  rows.push(settingRow('Editor colour theme', 'From an extension you installed in the Extensions side bar.', themeHost));

  // Custom background: picked in the native dialog and copied into the app's
  // own media folder (it used to be inlined as a data URL, which the settings
  // size cap rejected for any real photo).
  const bgControl = el('div', 'bg-control');
  const pickBtn = el('button', '', s.backgroundImage ? 'Change image' : 'Choose image…');
  const clearBtn = el('button', '', 'Remove');
  clearBtn.style.display = s.backgroundImage ? '' : 'none';
  const bgError = el('div', 'setting-error');
  bgError.style.display = 'none';
  const showBgError = (e: unknown): void => {
    bgError.textContent = e instanceof Error ? e.message : String(e);
    bgError.style.display = '';
  };
  pickBtn.addEventListener('click', () => {
    bgError.style.display = 'none';
    pickBtn.disabled = true;
    void window.void
      .pickMedia('image')
      .then(async (picked) => {
        if (!picked) return;
        await apply({ backgroundImage: picked, backgroundVideo: null });
        clearBtn.style.display = '';
        pickBtn.textContent = 'Change image';
        adjustBtn.disabled = false;
      })
      .catch(showBgError)
      .finally(() => { pickBtn.disabled = false; });
  });
  clearBtn.addEventListener('click', () => {
    void apply({ backgroundImage: null });
    clearBtn.style.display = 'none';
    pickBtn.textContent = 'Choose image…';
    adjustBtn.disabled = !state.settings.backgroundVideo;
  });
  bgControl.append(pickBtn, clearBtn, bgError);
  rows.push(settingRow('Custom background', 'Show an image behind the workbench. A copy is kept in the app\'s own folder on this machine (up to 50 MB).', bgControl));

  // Video background: picked via the native dialog (path reference, not inlined)
  // and played as a muted loop. Mutually exclusive with a background image.
  const vidControl = el('div', 'bg-control');
  const vidPickBtn = el('button', '', s.backgroundVideo ? 'Change video' : 'Choose video…');
  const vidClearBtn = el('button', '', 'Remove');
  vidClearBtn.style.display = s.backgroundVideo ? '' : 'none';
  const vidError = el('div', 'setting-error');
  vidError.style.display = 'none';
  vidPickBtn.addEventListener('click', () => {
    vidError.style.display = 'none';
    vidPickBtn.disabled = true;
    const label = vidPickBtn.textContent;
    vidPickBtn.textContent = 'Copying…';
    void window.void
      .pickMedia('video')
      .then(async (picked) => {
        if (!picked) return;
        // Picking a video replaces any image background.
        await apply({ backgroundVideo: picked, backgroundImage: null });
        vidClearBtn.style.display = '';
        adjustBtn.disabled = false;
      })
      .catch((e: unknown) => {
        vidError.textContent = e instanceof Error ? e.message : String(e);
        vidError.style.display = '';
      })
      .finally(() => {
        vidPickBtn.disabled = false;
        vidPickBtn.textContent = state.settings.backgroundVideo ? 'Change video' : label || 'Choose video…';
      });
  });
  vidClearBtn.addEventListener('click', () => {
    void apply({ backgroundVideo: null });
    vidClearBtn.style.display = 'none';
    vidPickBtn.textContent = 'Choose video…';
    adjustBtn.disabled = !state.settings.backgroundImage;
  });
  vidControl.append(vidPickBtn, vidClearBtn, vidError);
  rows.push(settingRow('Video background', 'Play a looping, muted video behind the workbench. A copy is kept in the app\'s own folder (up to 1 GB), so it stays after a restart.', vidControl));

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
      'Keep text readable over your background: light text with a soft shadow, and denser panels.',
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
  { id: 'sec-updates', label: 'Updates', build: buildUpdatesSection },
  { id: 'sec-cli', label: 'Command line', build: buildCliSection },
  { id: 'sec-mobile', label: 'Continue on mobile', build: buildMobileSection },
  { id: 'sec-ai', label: 'AI', build: buildAISection },
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
