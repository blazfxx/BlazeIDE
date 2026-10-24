// AI panel input area: mode pills, auto-growing textarea, send/cancel button.

import type { AIMode, ElementComment, FileNode, ReasoningEffort } from '../../shared/types';
import { REASONING_LEVELS } from '../../shared/types';
import { bus, el, escapeHtml, state } from '../state';
import { icon, iconAI, iconClose, iconSend } from '../icons/index';
import type { Controller } from './controller';

const PLACEHOLDERS: Record<AIMode, string> = {
  ask: 'Ask about your code...',
  plan: 'Describe what you want to build — Blaze will plan it...',
  agent: 'Give the agent a task...',
};

const REASONING_LABEL: Record<ReasoningEffort, string> = {
  off: 'Off',
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  max: 'Max',
};

export function initInput(host: HTMLElement, controller: Controller): void {
  const wrap = el('div', 'ai-input-wrap');

  const toolbar = el('div', 'ai-toolbar');
  const modeTabs = el('div', 'mode-tabs');
  const modes: AIMode[] = ['ask', 'plan', 'agent'];
  const tabEls = new Map<AIMode, HTMLElement>();
  // Sliding indicator that animates to the active tab (segmented control).
  const thumb = el('div', 'mode-thumb');
  modeTabs.appendChild(thumb);
  for (const mode of modes) {
    const tab = el('div', 'mode-tab', mode.charAt(0).toUpperCase() + mode.slice(1));
    tab.addEventListener('click', () => setMode(mode));
    tabEls.set(mode, tab);
    modeTabs.appendChild(tab);
  }

  // Reasoning effort dropdown — a compact pill that cycles isn't discoverable,
  // so use a native select styled to match.
  const reasoning = el('div', 'reasoning-pick');
  reasoning.appendChild(el('span', 'icon icon-12 reasoning-icon', iconAI));
  const reasoningSelect = el('select', 'reasoning-select');
  for (const level of REASONING_LEVELS) {
    const opt = el('option', '', REASONING_LABEL[level]);
    opt.value = level;
    reasoningSelect.appendChild(opt);
  }
  reasoningSelect.value = state.settings.reasoningEffort;
  reasoningSelect.title = 'Reasoning effort';
  reasoningSelect.addEventListener('change', () => {
    const v = reasoningSelect.value as ReasoningEffort;
    state.settings.reasoningEffort = v;
    void window.void.settingsSet({ reasoningEffort: v });
  });
  reasoning.appendChild(reasoningSelect);

  toolbar.append(modeTabs, reasoning);

  // The textarea and send button share one rounded "field" so the arrow sits
  // inside the input. The whole field grows with the textarea.
  const field = el('div', 'ai-input-field');
  const textarea = el('textarea', 'ai-textarea');
  textarea.rows = 1;
  textarea.spellcheck = false;
  const sendBtn = el('button', 'ai-send', icon(iconSend));
  sendBtn.title = 'Send (Enter)';
  field.append(textarea, sendBtn);

  // Queued element comments from the web preview, shown as removable chips
  // above the input and folded into the next message's context on send.
  const comments: ElementComment[] = [];
  const chips = el('div', 'ai-comment-chips');
  chips.style.display = 'none';

  function renderChips(): void {
    chips.textContent = '';
    if (comments.length === 0) { chips.style.display = 'none'; return; }
    chips.style.display = 'flex';
    for (const c of comments) {
      const chip = el('div', 'ai-comment-chip');
      const label = c.tag ? `<${escapeHtml(c.tag)}>` : 'element';
      chip.appendChild(el('span', 'chip-el', label));
      chip.appendChild(el('span', 'chip-text', escapeHtml(c.comment)));
      const x = el('button', 'chip-x', icon(iconClose, 'icon-12'));
      x.title = 'Remove';
      x.addEventListener('click', () => {
        const i = comments.findIndex((q) => q.id === c.id);
        if (i >= 0) comments.splice(i, 1);
        renderChips();
      });
      chip.appendChild(x);
      chips.appendChild(chip);
    }
  }

  bus.on('preview-comment', (c) => {
    comments.push(c);
    renderChips();
    bus.emit('focus-ai-input', undefined);
  });

  // --- @-mention / pinned files -------------------------------------------
  // Files pinned into context (via @mention or tree multi-select). Their
  // content is injected into the next message and shown as chips.
  const pinned: string[] = []; // project-relative paths
  const pinChips = el('div', 'ai-pin-chips');
  pinChips.style.display = 'none';

  function relPath(p: string): string {
    const root = state.projectRoot;
    return root && p.startsWith(root) ? p.slice(root.length).replace(/^\/+/, '') : p;
  }
  function pinFile(rel: string): void {
    if (!rel || pinned.includes(rel)) return;
    pinned.push(rel);
    renderPins();
    bus.emit('focus-ai-input', undefined);
  }
  function renderPins(): void {
    pinChips.textContent = '';
    if (pinned.length === 0) { pinChips.style.display = 'none'; return; }
    pinChips.style.display = 'flex';
    for (const rel of pinned) {
      const chip = el('div', 'ai-pin-chip');
      chip.appendChild(el('span', 'pin-at', '@'));
      chip.appendChild(el('span', 'pin-name', escapeHtml(rel.split('/').pop() ?? rel)));
      const x = el('button', 'chip-x', icon(iconClose, 'icon-12'));
      x.title = 'Unpin';
      x.addEventListener('click', () => {
        const i = pinned.indexOf(rel);
        if (i >= 0) pinned.splice(i, 1);
        renderPins();
      });
      chip.appendChild(x);
      chip.title = rel;
      pinChips.appendChild(chip);
    }
  }
  // Tree multi-select → "add to chat".
  bus.on('ai-pin-files', ({ paths }) => {
    for (const p of paths) pinFile(relPath(p));
  });

  // AI question panel: shown above the input when the agent asks something.
  const questionHost = el('div', 'ai-questions');
  questionHost.style.display = 'none';

  bus.on('ai-ask-question', ({ questions, resolve }) => {
    questionHost.textContent = '';
    questionHost.style.display = 'block';
    // Lock the composer: while a question is up, the toolbar + input field are
    // hidden so the user must answer here rather than typing/sending a message.
    wrap.classList.add('asking');
    const answers: string[] = new Array(questions.length).fill('');
    let step = 0; // index of the question currently shown

    // Render ONE question at a time (stepper), advancing on Next until the last.
    function renderStep(): void {
      questionHost.textContent = '';
      const q = questions[step];

      const header = el('div', 'ai-q-header');
      header.appendChild(el('span', 'icon icon-12 ai-q-mark', iconAI));
      const label = questions.length > 1 ? `Question ${step + 1} of ${questions.length}` : 'Quick question';
      header.appendChild(el('span', 'ai-q-header-text', label));
      questionHost.appendChild(header);

      const block = el('div', 'ai-q');
      block.appendChild(el('div', 'ai-q-text', escapeHtml(q.question)));
      const opts = el('div', 'ai-q-opts');
      const custom = el('input', 'ai-q-custom') as HTMLInputElement;
      custom.placeholder = 'Type your own answer…';

      const isLast = step === questions.length - 1;
      const advance = (): void => {
        if (answers[step].trim().length === 0) return;
        if (isLast) {
          questionHost.style.display = 'none';
          questionHost.textContent = '';
          wrap.classList.remove('asking');
          resolve(answers);
        } else {
          step += 1;
          renderStep();
        }
      };

      const markAnswered = (val: string, btn?: HTMLElement): void => {
        answers[step] = val;
        opts.querySelectorAll('.ai-q-opt').forEach((b) => b.classList.remove('chosen'));
        if (btn) btn.classList.add('chosen');
        nextBtn.disabled = val.trim().length === 0;
      };

      (q.options ?? []).forEach((o) => {
        const b = el('button', 'ai-q-opt', escapeHtml(o));
        b.addEventListener('click', () => {
          custom.value = '';
          markAnswered(o, b);
          // Select only — the user confirms with Next/Send (safety against
          // mis-clicks).
        });
        opts.appendChild(b);
      });
      custom.addEventListener('input', () => {
        opts.querySelectorAll('.ai-q-opt').forEach((b) => b.classList.remove('chosen'));
        markAnswered(custom.value);
      });
      custom.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); advance(); }
      });
      if ((q.options ?? []).length > 0) block.appendChild(opts);
      block.appendChild(custom);
      questionHost.appendChild(block);

      const sendRow = el('div', 'ai-q-send-row');
      if (step > 0) {
        const backBtn = el('button', 'ai-q-back', 'Back');
        backBtn.addEventListener('click', () => { step -= 1; renderStep(); });
        sendRow.appendChild(backBtn);
      }
      const nextBtn = el('button', 'primary ai-q-send', isLast ? 'Send' : 'Next');
      nextBtn.disabled = answers[step].trim().length === 0;
      nextBtn.addEventListener('click', advance);
      sendRow.appendChild(nextBtn);
      questionHost.appendChild(sendRow);

      // Restore a previously typed custom answer when navigating back.
      if (answers[step] && !(q.options ?? []).includes(answers[step])) custom.value = answers[step];
      bus.emit('focus-ai-input', undefined);
    }

    renderStep();
  });

  wrap.append(questionHost, chips, pinChips, toolbar, field);
  host.appendChild(wrap);

  // --- @-mention autocomplete dropdown ---
  const mentionMenu = el('div', 'ai-mention-menu');
  mentionMenu.style.display = 'none';
  field.appendChild(mentionMenu);
  let mentionItems: string[] = []; // rel paths currently shown
  let mentionIndex = 0;
  let fileList: string[] = []; // cached project file list (rel paths)

  async function ensureFileList(): Promise<void> {
    const root = state.projectRoot;
    if (!root) { fileList = []; return; }
    try {
      const tree = await window.void.fsReadTree(root);
      const out: string[] = [];
      const walk = (node: FileNode): void => {
        for (const c of node.children ?? []) {
          if (['node_modules', '.git', '.blazeide', 'dist', 'target'].includes(c.name)) continue;
          if (c.isDirectory) walk(c);
          else out.push(relPath(c.path));
          if (out.length > 5000) return;
        }
      };
      walk(tree);
      fileList = out;
    } catch { fileList = []; }
  }

  /** The @token immediately before the cursor, or null. */
  function activeMention(): { query: string; start: number } | null {
    const pos = textarea.selectionStart ?? textarea.value.length;
    const upto = textarea.value.slice(0, pos);
    const m = /(^|\s)@([^\s@]*)$/.exec(upto);
    if (!m) return null;
    return { query: m[2], start: pos - m[2].length - 1 };
  }

  function hideMention(): void { mentionMenu.style.display = 'none'; mentionItems = []; }

  function renderMention(): void {
    const m = activeMention();
    if (!m) { hideMention(); return; }
    const q = m.query.toLowerCase();
    mentionItems = fileList
      .filter((f) => f.toLowerCase().includes(q))
      .sort((a, b) => a.length - b.length)
      .slice(0, 8);
    if (mentionItems.length === 0) { hideMention(); return; }
    mentionIndex = 0;
    mentionMenu.textContent = '';
    mentionItems.forEach((rel, i) => {
      const row = el('div', `ai-mention-item${i === 0 ? ' active' : ''}`);
      row.appendChild(el('span', 'mention-name', escapeHtml(rel.split('/').pop() ?? rel)));
      const dir = rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '';
      if (dir) row.appendChild(el('span', 'mention-dir', escapeHtml(dir)));
      row.addEventListener('mousedown', (e) => { e.preventDefault(); chooseMention(i); });
      mentionMenu.appendChild(row);
    });
    mentionMenu.style.display = 'block';
  }

  function chooseMention(i: number): void {
    const m = activeMention();
    const rel = mentionItems[i];
    if (!m || !rel) return;
    // Remove the "@query" the user typed and pin the file instead.
    const pos = textarea.selectionStart ?? textarea.value.length;
    textarea.value = textarea.value.slice(0, m.start) + textarea.value.slice(pos);
    pinFile(rel);
    hideMention();
    autoGrow();
    textarea.focus();
  }

  function renderTabs(): void {
    for (const mode of modes) {
      const tab = tabEls.get(mode);
      if (!tab) continue;
      tab.classList.toggle('active', state.settings.aiMode === mode);
      // Agent mode is available on all plans — no lock.
    }
    positionThumb();
    textarea.placeholder = PLACEHOLDERS[state.settings.aiMode];
    if (reasoningSelect.value !== state.settings.reasoningEffort) {
      reasoningSelect.value = state.settings.reasoningEffort;
    }
  }

  // Slide the indicator to sit exactly under the active tab.
  function positionThumb(): void {
    const active = tabEls.get(state.settings.aiMode);
    if (!active) return;
    thumb.style.width = `${active.offsetWidth}px`;
    thumb.style.transform = `translateX(${active.offsetLeft}px)`;
  }

  function setMode(mode: AIMode): void {
    if (mode === state.settings.aiMode) return;
    state.settings.aiMode = mode;
    void window.void.settingsSet({ aiMode: mode });
    bus.emit('ai-mode-changed', { mode });
    renderTabs();
  }

  function autoGrow(): void {
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }

  function renderSendButton(): void {
    if (controller.isStreaming()) {
      sendBtn.innerHTML = '<span class="cancel-square"></span>';
      sendBtn.title = 'Stop';
      sendBtn.classList.add('streaming');
    } else {
      sendBtn.innerHTML = icon(iconSend);
      sendBtn.title = 'Send (Enter)';
      sendBtn.classList.remove('streaming');
    }
  }

  function buildCommentBlock(): string {
    if (comments.length === 0) return '';
    const parts = comments.map((c, i) => {
      const lines = [
        `[Element ${i + 1}] target: ${c.tag ? `<${c.tag}>` : 'element'}${c.selector ? ` (selector: ${c.selector})` : ''}`,
        c.html ? `opening tag: ${c.html}` : '',
        c.text ? `its text: "${c.text}"` : '',
        `requested change: ${c.comment}`,
      ].filter(Boolean);
      return lines.join('\n');
    });
    return [
      'The user selected element(s) in the live preview and requested changes. Apply each change to the SELECTED element itself (identified by its tag/selector) — not to its parent or to child elements inside it. If the user selected a container like <body> or a wrapper, edit that element (e.g. its own background/layout), not the things inside it.',
      '',
      parts.join('\n\n'),
      '',
    ].join('\n');
  }

  function submit(): void {
    if (controller.isStreaming()) {
      controller.cancel();
      return;
    }
    const text = textarea.value.trim();
    // Allow sending with only comments or pinned files queued.
    if (!text && comments.length === 0 && pinned.length === 0) return;
    const block = buildCommentBlock();
    const pins = pinned.slice();
    // Capture display notes BEFORE clearing the queues.
    const notes: string[] = [];
    if (comments.length > 0) {
      const each = comments.map((c) => `• ${c.comment}`).join('\n');
      notes.push(`Commenting on ${comments.length} selected element${comments.length === 1 ? '' : 's'}:\n${each}`);
    }
    if (pins.length > 0) notes.push(`📎 ${pins.length} file${pins.length === 1 ? '' : 's'}: ${pins.map((p) => p.split('/').pop()).join(', ')}`);
    textarea.value = '';
    comments.length = 0;
    pinned.length = 0;
    renderChips();
    renderPins();
    hideMention();
    autoGrow();
    // Build the message asynchronously so we can read pinned file contents.
    void (async () => {
      let fileContext = '';
      for (const rel of pins) {
        const root = state.projectRoot;
        if (!root) break;
        const content = await window.void.fsReadFile(`${root}/${rel}`).catch(() => null);
        if (content !== null) {
          const capped = content.length > 24_000 ? `${content.slice(0, 24_000)}\n...[truncated]` : content;
          fileContext += `\n--- ${rel} ---\n${capped}\n`;
        }
      }
      const ctxBlock = fileContext ? `The user attached these files for context:\n${fileContext}\n` : '';
      const message = `${ctxBlock}${block}${block && text ? '\n' : ''}${text || (block ? 'Apply the changes described in the comments above.' : 'See the attached files.')}`;
      const display = text ? (notes.length ? `${text}\n\n${notes.join('\n')}` : text) : notes.join('\n');
      await controller.send(message, display).finally(renderSendButton);
    })();
    renderSendButton();
  }

  textarea.addEventListener('input', () => { autoGrow(); renderMention(); });
  textarea.addEventListener('keydown', (e) => {
    // When the @-mention menu is open, arrows/enter/tab drive it.
    if (mentionMenu.style.display === 'block' && mentionItems.length > 0) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        mentionIndex = e.key === 'ArrowDown'
          ? (mentionIndex + 1) % mentionItems.length
          : (mentionIndex - 1 + mentionItems.length) % mentionItems.length;
        [...mentionMenu.children].forEach((c, i) => c.classList.toggle('active', i === mentionIndex));
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); chooseMention(mentionIndex); return; }
      if (e.key === 'Escape') { e.preventDefault(); hideMention(); return; }
    }
    // Enter sends; Shift+Enter (or ⌘+Enter) inserts a newline.
    if (e.key === 'Enter' && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      submit();
    }
  });
  textarea.addEventListener('blur', () => window.setTimeout(hideMention, 120));
  // Load the file list once a project is open (and refresh on change).
  bus.on('project-opened', () => void ensureFileList());
  window.void.onFsChanged(() => void ensureFileList());
  void ensureFileList();
  sendBtn.addEventListener('click', submit);

  bus.on('ai-status', renderSendButton);
  bus.on('session-changed', renderTabs);
  bus.on('settings-changed', renderTabs);
  // Mode can change programmatically (e.g. Plan → Agent build handoff).
  bus.on('ai-mode-changed', renderTabs);
  bus.on('focus-ai-input', () => {
    // Layout listens to this too and un-collapses the panel first.
    textarea.focus();
  });
  // Reposition the indicator if the panel is resized.
  window.addEventListener('resize', positionThumb);

  renderTabs();
  // Tabs may have zero width until first layout; place the thumb (without a
  // visible slide) once measurements are real.
  requestAnimationFrame(() => {
    modeTabs.classList.add('ready');
    positionThumb();
  });
}
