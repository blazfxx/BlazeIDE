// BlBots tab: your roster of AI bots, their chats, rooms where bots work
// together, routines and each bot's private computer.
// All dynamic text is set with textContent (el() writes innerHTML).

import { el } from '../state';
import type { EditorArea } from '../editor/index';
import type { BotProfile, BotsOverview } from '../../shared/ipc';
import { botAvatar, openBotEditor, TEMPLATES, type Template } from './create';
import { faceSvg, FACE_COLORS } from './faces';
import { openSkillsLibrary } from '../skills/library';

type Target = { kind: 'bot'; id: string; thread: string | null } | { kind: 'room'; id: string } | null;

function t(tag: keyof HTMLElementTagNameMap, cls: string, text?: string): HTMLElement {
  const e = el(tag, cls);
  if (text !== undefined) e.textContent = text;
  return e;
}

function avatar(b: { avatar: string; color: string; name: string; face?: number }, size: 'sm' | 'md' | 'lg' = 'md'): HTMLElement {
  const a = botAvatar(b, size === 'sm' ? 30 : size === 'md' ? 42 : 96);
  a.classList.add(size);
  return a;
}

export function openBlBots(editor: EditorArea): void {
  editor.openCustomTab('blbots', 'BlBots', (host) => render(host));
}

function when(ms?: number): string {
  if (!ms) return '';
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  const days = Math.floor((now.getTime() - d.getTime()) / 86_400_000);
  return days < 7 ? d.toLocaleDateString([], { weekday: 'short' }) : d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

function render(host: HTMLElement): void {
  host.textContent = '';
  const root = el('div', 'bb');
  const side = el('div', 'bb-side');
  const main = el('div', 'bb-main');
  root.append(side, main);
  host.appendChild(root);

  let data: BotsOverview = { bots: [], rooms: [], engines: [] };
  let target: Target = null;
  let runId: string | null = null;
  let chatId: string | null = null;
  let liveEl: HTMLElement | null = null;
  let liveText = '';
  let liveBot = '';
  let msgs: HTMLElement | null = null;
  let sendBtn: HTMLButtonElement | null = null;

  const unsubs: Array<() => void> = [];
  const alive = (): boolean => root.isConnected;

  async function load(): Promise<void> {
    data = await window.void.botsList();
    drawSide();
  }

  let search = '';
  function drawSide(): void {
    const focused = document.activeElement?.classList.contains('bb-search');
    side.textContent = '';
    const head = el('div', 'bb-side-head');
    head.appendChild(t('div', 'bb-brand', 'BlBots'));
    const nb = t('button', 'bb-round', '+') as HTMLButtonElement;
    nb.title = 'New bot';
    nb.addEventListener('click', () => editBot(null));
    const sk = t('button', 'bb-skills', 'Skills') as HTMLButtonElement;
    sk.title = 'Skills library: import, add and edit skills';
    sk.addEventListener('click', () => openSkillsLibrary());
    const actions = el('span', 'bb-side-actions');
    actions.append(sk, nb);
    head.appendChild(actions);
    side.appendChild(head);
    const sbox = el('input', 'bb-search') as HTMLInputElement;
    sbox.placeholder = 'Search';
    sbox.value = search;
    sbox.addEventListener('input', () => { search = sbox.value; drawSide(); });
    side.appendChild(sbox);
    if (focused) { sbox.focus(); sbox.setSelectionRange(search.length, search.length); }
    const q = search.trim().toLowerCase();
    const list = el('div', 'bb-list');
    side.appendChild(list);
    const bots = data.bots.filter((x) => !x.hidden && (!q || x.name.toLowerCase().includes(q) || x.title.toLowerCase().includes(q)));
    if (!data.bots.length) {
      const e = el('div', 'bb-side-empty');
      e.append(t('div', 'bb-side-empty-title', 'No bots yet'), t('div', '', 'Your bots will show up here.'));
      list.appendChild(e);
    }
    for (const b of bots) {
      const row = el('button', 'bb-item');
      if (target?.kind === 'bot' && target.id === b.id) row.classList.add('active');
      const txt = el('span', 'bb-item-text');
      const top = el('span', 'bb-item-top');
      top.append(t('span', 'bb-item-name', (b.pinned ? '📌 ' : '') + b.name), t('span', 'bb-item-time', when(b.lastAt)));
      txt.append(top, t('span', 'bb-item-sub', b.lastTitle || b.title || engineLabel(b)));
      row.append(avatar(b), txt);
      row.addEventListener('click', () => void openBot(b.id, null));
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); botMenu(b, e.clientX, e.clientY); });
      list.appendChild(row);
    }
    const rooms = data.rooms.filter((r) => !q || r.name.toLowerCase().includes(q));
    if (data.bots.length) {
      const rh = el('div', 'bb-section');
      rh.appendChild(t('span', '', 'Rooms'));
      const nr = t('button', 'bb-round sm', '+') as HTMLButtonElement;
      nr.title = 'New room';
      nr.addEventListener('click', () => editRoom(null));
      rh.appendChild(nr);
      list.appendChild(rh);
      if (!data.rooms.length) list.appendChild(t('div', 'bb-hint', 'Put several bots in one chat: @mention one, or let the lead hand out the work.'));
    }
    for (const r of rooms) {
      const row = el('button', 'bb-item');
      if (target?.kind === 'room' && target.id === r.id) row.classList.add('active');
      const txt = el('span', 'bb-item-text');
      const top = el('span', 'bb-item-top');
      top.append(t('span', 'bb-item-name', r.name), t('span', 'bb-item-time', when(r.updated * 1000)));
      const names = r.members.map((m) => data.bots.find((b) => b.id === m)?.name).filter(Boolean).join(', ');
      txt.append(top, t('span', 'bb-item-sub', names));
      row.append(t('span', 'bb-avatar md room', '#'), txt);
      row.addEventListener('click', () => void openRoom(r.id));
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); roomMenu(r.id, e.clientX, e.clientY); });
      list.appendChild(row);
    }
  }

  function engineLabel(b: BotProfile): string {
    const k = b.engine.kind;
    return k === 'claude' ? 'Claude Code' : k === 'codex' ? 'Codex' : k === 'grok' ? 'Grok CLI' : (b.model || 'BlazeAPI');
  }

  function menu(x: number, y: number, items: Array<[string, () => void]>): void {
    document.querySelector('.bb-menu')?.remove();
    const m = el('div', 'bb-menu');
    m.style.left = `${x}px`;
    m.style.top = `${y}px`;
    for (const [label, fn] of items) {
      const b = t('button', '', label);
      b.addEventListener('click', () => { m.remove(); fn(); });
      m.appendChild(b);
    }
    document.body.appendChild(m);
    setTimeout(() => document.addEventListener('click', () => m.remove(), { once: true }), 0);
  }

  function botMenu(b: BotProfile, x: number, y: number): void {
    menu(x, y, [
      ['Edit', () => editBot(b)],
      [b.pinned ? 'Unpin' : 'Pin', () => void window.void.botsSave({ ...b, pinned: !b.pinned }).then(load)],
      ['Duplicate', () => void window.void.botsDuplicate(b.id).then(load)],
      ['Copy id', () => void navigator.clipboard.writeText(b.id)],
      [b.hidden ? 'Show' : 'Hide', () => void window.void.botsSave({ ...b, hidden: !b.hidden }).then(load)],
      ['Delete…', () => { if (confirm(`Delete ${b.name}, its chats and memory? Its working folder is kept.`)) void window.void.botsDelete(b.id).then(() => load()).then(() => drawWelcome()); }],
    ]);
  }

  function roomMenu(id: string, x: number, y: number): void {
    const r = data.rooms.find((z) => z.id === id);
    menu(x, y, [
      ['Edit', () => r && editRoom(r)],
      ['Delete…', () => { if (confirm('Delete this room and its history?')) void window.void.roomsDelete(id).then(() => { target = null; main.textContent = ''; return load(); }); }],
    ]);
  }

  // ---------- welcome / first bot ----------
  function drawWelcome(): void {
    main.textContent = '';
    target = null;
    const w = el('div', 'bbh');
    const faces = el('div', 'bbh-faces');
    [[2, FACE_COLORS[3]], [1, FACE_COLORS[4]], [7, FACE_COLORS[0]], [5, FACE_COLORS[5]], [6, FACE_COLORS[2]]].forEach(([f, c], i) => {
      const s = el('span', `bbh-face f${i}`);
      s.innerHTML = faceSvg(f as number, c as string, i === 2 ? 92 : 64);
      faces.appendChild(s);
    });
    const first = !data.bots.length;
    w.append(
      faces,
      t('div', 'bbh-title', first ? 'Meet BlBots' : 'Make another bot'),
      t('div', 'bbh-sub', first
        ? 'AI teammates with their own name, personality, folder and memory. They code, research and write on this computer, and you can reach them from the CLI and your phone.'
        : 'Start from one of these, or from scratch.'),
    );
    const grid = el('div', 'bbh-grid');
    for (const tp of TEMPLATES) {
      const c = el('button', 'bbh-card');
      const av = el('span', 'bbh-card-face');
      av.innerHTML = faceSvg(tp.bot.face ?? 1, tp.bot.color ?? FACE_COLORS[4], 46);
      const tx = el('span', 'bbh-card-text');
      tx.append(t('span', 'bbh-card-name', tp.label), t('span', 'bbh-card-sub', tp.blurb));
      c.append(av, tx);
      c.addEventListener('click', () => editBot(null, tp));
      grid.appendChild(c);
    }
    const blank = el('button', 'bbh-card blank');
    const plus = t('span', 'bbh-card-face plus', '+');
    const bt = el('span', 'bbh-card-text');
    bt.append(t('span', 'bbh-card-name', 'Start from scratch'), t('span', 'bbh-card-sub', 'Pick a face, a name and a personality.'));
    blank.append(plus, bt);
    blank.addEventListener('click', () => editBot(null));
    grid.appendChild(blank);
    w.appendChild(grid);
    const foot = el('div', 'bbh-foot');
    foot.append(t('span', '', 'Runs on BlazeAPI models, or on Claude Code, Codex or Grok with your own plan.'));
    w.appendChild(foot);
    main.appendChild(w);
  }

  // ---------- conversation view ----------
  function frame(title: string, sub: string, actions: HTMLElement[], who?: HTMLElement): HTMLElement {
    main.textContent = '';
    const head = el('div', 'bb-head');
    if (who) head.appendChild(who);
    const h = el('div', 'bb-head-text');
    h.append(t('div', 'bb-head-title', title), t('div', 'bb-head-sub', sub));
    head.appendChild(h);
    const acts = el('div', 'bb-head-actions');
    for (const a of actions) acts.appendChild(a);
    head.appendChild(acts);
    msgs = el('div', 'bb-msgs');
    const comp = el('div', 'bb-composer');
    const pill = el('div', 'bb-pill');
    const ta = el('textarea') as HTMLTextAreaElement;
    ta.rows = 1;
    ta.placeholder = target?.kind === 'room' ? 'Message the room · @Name to address a bot' : `Message ${title}`;
    ta.addEventListener('input', () => { ta.style.height = 'auto'; ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`; });
    sendBtn = t('button', 'bb-send', '↑') as HTMLButtonElement;
    sendBtn.title = 'Send (Enter)';
    const send = (): void => {
      if (runId) { void window.void.botsCancel(runId); return; }
      const text = ta.value.trim();
      if (!text || !target) return;
      ta.value = '';
      ta.style.height = 'auto';
      main.querySelector('.bb-welcome')?.remove();
      addMsg({ role: 'user', text });
      const p = target.kind === 'bot' ? window.void.botsSend(target.id, target.thread, text) : window.void.roomsSend(target.id, text);
      void p.then((r) => {
        runId = r.runId;
        chatId = r.chatId;
        if (target?.kind === 'bot') target.thread = r.chatId;
        setWorking(true);
      }).catch((e: unknown) => addNote(String(e instanceof Error ? e.message : e), true));
    };
    sendBtn.addEventListener('click', send);
    ta.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); } });
    pill.append(ta, sendBtn);
    comp.appendChild(pill);
    main.append(head, msgs, comp);
    setTimeout(() => ta.focus(), 0);
    return msgs;
  }

  function setWorking(on: boolean): void {
    if (sendBtn) { sendBtn.textContent = on ? '■' : '↑'; sendBtn.title = on ? 'Stop' : 'Send (Enter)'; }
    main.classList.toggle('bb-working', on);
  }

  function scrollDown(): void { if (msgs) msgs.scrollTop = msgs.scrollHeight; }

  function addMsg(m: { role: string; text: string; tools?: string[]; author?: string }): void {
    if (!msgs) return;
    const row = el('div', `bb-row ${m.role}`);
    if (m.role !== 'user' && target?.kind === 'room' && m.author) {
      const b = data.bots.find((x) => x.name === m.author);
      row.appendChild(b ? avatar(b, 'sm') : t('span', 'bb-avatar sm', m.author.slice(0, 1)));
    }
    const col = el('div', 'bb-col');
    if (m.author && m.role !== 'user') col.appendChild(t('div', 'bb-author', m.author));
    for (const tool of m.tools ?? []) col.appendChild(t('div', 'bb-tool', tool));
    if (m.text?.trim()) col.appendChild(t('div', 'bb-bubble', m.text));
    row.appendChild(col);
    msgs.appendChild(row);
    scrollDown();
  }

  function addNote(text: string, bad = false): void {
    msgs?.appendChild(t('div', bad ? 'bb-note bad' : 'bb-note', text));
    scrollDown();
  }

  function addCard(p: { id: string; kind: string; summary?: string; questions?: Array<{ question: string; options: string[] }> }): void {
    if (!msgs || document.getElementById(`bbc-${p.id}`)) return;
    const c = el('div', 'bb-card');
    c.id = `bbc-${p.id}`;
    if (p.kind === 'approve') {
      c.append(t('div', 'bb-card-q', 'Allow this?'), t('div', '', p.summary ?? ''));
      const row = el('div', 'bb-card-row');
      const no = t('button', '', 'Refuse');
      const yes = t('button', 'primary', 'Allow');
      no.addEventListener('click', () => void window.void.botsAnswer(p.id, false, null).finally(() => c.remove()));
      yes.addEventListener('click', () => void window.void.botsAnswer(p.id, true, null).finally(() => c.remove()));
      row.append(no, yes);
      c.appendChild(row);
    } else {
      const answers: string[] = [];
      const qs = p.questions ?? [];
      const inputs = qs.map((q) => {
        c.appendChild(t('div', 'bb-card-q', q.question));
        const i = el('input') as HTMLInputElement;
        i.placeholder = q.options.length ? q.options.join(' / ') : 'Your answer';
        c.appendChild(i);
        return i;
      });
      const ok = t('button', 'primary', 'Answer');
      ok.addEventListener('click', () => { inputs.forEach((i) => answers.push(i.value.trim())); void window.void.botsAnswer(p.id, null, answers).finally(() => c.remove()); });
      c.appendChild(ok);
    }
    msgs.appendChild(c);
    scrollDown();
  }

  async function openBot(id: string, thread: string | null): Promise<void> {
    const b = data.bots.find((x) => x.id === id);
    if (!b) return;
    target = { kind: 'bot', id, thread };
    runId = null;
    liveEl = null;
    drawSide();
    const info = await window.void.botsThreads(id);
    const sel = el('select', 'bb-threads') as HTMLSelectElement;
    const optNew = el('option') as HTMLOptionElement;
    optNew.value = '';
    optNew.textContent = 'New chat';
    sel.appendChild(optNew);
    for (const th of info.threads) {
      const o = el('option') as HTMLOptionElement;
      o.value = th.id;
      o.textContent = th.title;
      if (th.id === thread) o.selected = true;
      sel.appendChild(o);
    }
    sel.addEventListener('change', () => void openBot(id, sel.value || null));
    sel.classList.add('bb-chip');
    const edit = t('button', 'bb-chip', 'Edit') as HTMLButtonElement;
    edit.addEventListener('click', () => editBot(b));
    const model = t('span', 'bb-chip static', b.engine.kind === 'blaze' ? (b.model || 'Default model') : engineLabel(b));
    const actions: HTMLElement[] = [model, sel, edit];
    if (b.computer) {
      const comp = t('button', 'bb-chip', 'Computer') as HTMLButtonElement;
      comp.addEventListener('click', () => {
        comp.disabled = true;
        comp.textContent = 'Starting…';
        void window.void.botsComputer(b.id).then((url) => openComputer(b.name, url)).catch((e: unknown) => addNote(String(e instanceof Error ? e.message : e), true)).finally(() => { comp.disabled = false; comp.textContent = 'Computer'; });
      });
      actions.push(comp);
    }
    frame(b.name, `${b.title ? b.title + ' · ' : ''}${b.approval === 'full' ? 'YOLO' : b.approval === 'auto' ? 'Auto' : b.approval === 'edits' ? 'Approves edits' : 'Asks first'}`, actions, avatar(b, 'sm'));
    if (thread) {
      chatId = thread;
      const th = await window.void.botsThread(id, thread);
      th.messages.forEach(addMsg);
      th.pending.forEach(addCard);
    } else {
      chatId = null;
      const w = el('div', 'bb-welcome');
      w.append(avatar(b, 'lg'), t('div', 'bb-welcome-name', b.name), t('div', 'bb-welcome-sub', b.title || engineLabel(b)));
      const mem = info.memory.split('\n').map((x) => x.replace(/^[-*]\s*/, '').trim()).filter(Boolean).slice(-6);
      if (mem.length) {
        const chips = el('div', 'bb-mem');
        chips.appendChild(t('div', 'bb-mem-title', 'Remembers'));
        for (const m of mem) chips.appendChild(t('span', 'bb-mem-chip', m));
        w.appendChild(chips);
      }
      const starters = el('div', 'bb-starters');
      for (const s of b.mode === 'ask'
        ? ['What can you help me with?', 'Explain how this project is put together', 'What should I learn next?']
        : ['What can you do?', 'Look around this folder and tell me what it is', 'Find one thing worth improving and do it']) {
        const c = t('button', 'bb-starter', s);
        c.addEventListener('click', () => {
          const ta = main.querySelector('.bb-pill textarea') as HTMLTextAreaElement | null;
          if (!ta) return;
          ta.value = s;
          ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        });
        starters.appendChild(c);
      }
      w.appendChild(starters);
      msgs?.appendChild(w);
    }
  }

  async function openRoom(id: string): Promise<void> {
    target = { kind: 'room', id };
    runId = null;
    liveEl = null;
    drawSide();
    const r = await window.void.roomsGet(id);
    chatId = `room:${id}`;
    const room = data.rooms.find((x) => x.id === id);
    const names = (room?.members ?? []).map((m) => data.bots.find((b) => b.id === m)?.name ?? '?');
    const edit = t('button', 'bb-chip', 'Edit') as HTMLButtonElement;
    edit.addEventListener('click', () => room && editRoom(room));
    frame(r.name, names.join(', '), [edit], t('span', 'bb-avatar sm room', '#'));
    r.messages.forEach(addMsg);
    r.pending.forEach(addCard);
    if (!r.messages.length) addNote('Write to the room. @Name addresses a bot; otherwise the lead answers and may hand work to the others.');
  }

  function openComputer(name: string, url: string): void {
    const ov = el('div', 'bb-overlay');
    const box = el('div', 'bb-computer');
    const bar = el('div', 'bb-head');
    bar.append(t('div', 'bb-head-title', `${name}'s computer`));
    const close = t('button', '', 'Close');
    close.addEventListener('click', () => ov.remove());
    bar.appendChild(close);
    const f = el('iframe') as HTMLIFrameElement;
    f.src = url;
    f.title = `${name}'s computer`;
    box.append(bar, f);
    ov.appendChild(box);
    document.body.appendChild(ov);
  }

  // ---------- live events ----------
  unsubs.push(window.void.onBotEvent((e) => {
    if (!alive()) return;
    if (e.chatId !== chatId && !(target?.kind === 'bot' && e.runId === runId)) return;
    const ev = e.ev;
    if (ev.type === 'text') {
      if (liveEl && target?.kind === 'room' && liveBot !== e.bot) liveEl = null;
      if (!liveEl && msgs) {
        liveEl = el('div', 'bb-row assistant');
        const who = data.bots.find((b) => b.id === e.bot);
        if (target?.kind === 'room' && who) liveEl.appendChild(avatar(who, 'sm'));
        const col = el('div', 'bb-col');
        if (target?.kind === 'room') col.appendChild(t('div', 'bb-author', who?.name ?? 'bot'));
        col.appendChild(t('div', 'bb-bubble', ''));
        liveEl.appendChild(col);
        msgs.appendChild(liveEl);
        liveText = '';
        liveBot = e.bot;
      }
      liveText += ev.text;
      const body = liveEl?.querySelector('.bb-bubble');
      if (body) body.textContent = liveText;
      scrollDown();
    } else if (ev.type === 'tool_start') {
      liveEl = null;
      msgs?.appendChild(t('div', 'bb-tool run', ev.summary));
      scrollDown();
    } else if (ev.type === 'tool_end' && ev.is_error) {
      addNote(`✗ ${String(ev.result).split('\n')[0].slice(0, 200)}`, true);
    } else if (ev.type === 'file_changed') {
      msgs?.appendChild(t('div', 'bb-tool', `${ev.created ? 'Created' : 'Edited'} ${ev.rel}`));
    } else if (ev.type === 'error') {
      addNote(ev.message, true);
    } else if (ev.type === 'notice') {
      addNote(ev.text);
    }
  }));
  unsubs.push(window.void.onBotPending((p) => { if (alive() && p.chatId === chatId) addCard(p); }));
  unsubs.push(window.void.onBotRunEnd((e) => {
    if (!alive() || e.runId !== runId) return;
    runId = null;
    liveEl = null;
    setWorking(false);
    if (e.error) addNote(e.error, true);
    if (e.stopped) addNote('Stopped.');
    if (target?.kind === 'bot') void window.void.botsThreads(target.id);
  }));
  unsubs.push(window.void.onBotSched((e) => {
    if (!alive()) return;
    if (e.kind === 'finished' && target?.kind === 'bot' && target.id === e.bot && target.thread === e.thread) void openBot(e.bot, e.thread);
  }));
  const gone = new MutationObserver(() => { if (!alive()) { unsubs.forEach((u) => u()); gone.disconnect(); } });
  gone.observe(document.body, { childList: true, subtree: true });

  // ---------- editors ----------
  function dialog(title: string, body: HTMLElement, onSave: () => Promise<void>): void {
    const ov = el('div', 'bb-overlay');
    const box = el('div', 'bb-dialog');
    const err = t('div', 'setting-error', '');
    const row = el('div', 'bb-card-row');
    const cancel = t('button', '', 'Cancel');
    const save = t('button', 'primary', 'Save') as HTMLButtonElement;
    const close = (): void => { ov.remove(); document.removeEventListener('keydown', onKey, true); };
    const onKey = (e: KeyboardEvent): void => { if (e.key === 'Escape') close(); };
    cancel.addEventListener('click', close);
    save.addEventListener('click', () => {
      save.disabled = true;
      err.textContent = '';
      void onSave().then(close).catch((e: unknown) => { err.textContent = String(e instanceof Error ? e.message : e); }).finally(() => { save.disabled = false; });
    });
    row.append(cancel, save);
    box.append(t('div', 'bb-head-title', title), body, err, row);
    ov.appendChild(box);
    // Esc or a click outside the dialog closes it.
    document.addEventListener('keydown', onKey, true);
    ov.addEventListener('mousedown', (e) => { if (e.target === ov) close(); });
    document.body.appendChild(ov);
  }

  function field(label: string, input: HTMLElement, help?: string): HTMLElement {
    const f = el('label', 'bb-field');
    f.appendChild(t('span', 'bb-field-label', label));
    f.appendChild(input);
    if (help) f.appendChild(t('span', 'bb-field-help', help));
    return f;
  }

  function input(value: string, placeholder = ''): HTMLInputElement {
    const i = el('input') as HTMLInputElement;
    i.value = value;
    i.placeholder = placeholder;
    return i;
  }

  function select(options: Array<[string, string]>, value: string): HTMLSelectElement {
    const s = el('select') as HTMLSelectElement;
    for (const [v, l] of options) {
      const o = el('option') as HTMLOptionElement;
      o.value = v;
      o.textContent = l;
      s.appendChild(o);
    }
    s.value = value;
    return s;
  }

  function editBot(b: BotProfile | null, template?: Template): void {
    const open = (memory: string): void => openBotEditor({
      bot: b,
      template,
      engines: data.engines,
      memory,
      onSave: async (p, mem) => {
        const saved = await window.void.botsSave(p);
        if (mem !== null) await window.void.botsSetMemory(saved.id, mem);
        await load();
        await openBot(saved.id, null);
      },
    });
    if (b) void window.void.botsThreads(b.id).then((i) => open(i.memory), () => open(''));
    else open('');
  }

  function editRoom(r: BotsOverview['rooms'][number] | null): void {
    const body = el('div', 'bb-form');
    const name = input(r?.name ?? '', 'Launch');
    const members = el('div', 'bb-members');
    const boxes: Array<[string, HTMLInputElement]> = [];
    for (const b of data.bots) {
      const l = el('label', 'bb-inline');
      const c = el('input') as HTMLInputElement;
      c.type = 'checkbox';
      c.checked = r ? r.members.includes(b.id) : false;
      l.append(c, t('span', '', b.name));
      members.appendChild(l);
      boxes.push([b.id, c]);
    }
    const lead = select(data.bots.map((b) => [b.id, b.name] as [string, string]), r?.lead ?? data.bots[0]?.id ?? '');
    const instr = el('textarea') as HTMLTextAreaElement;
    instr.rows = 4;
    instr.value = r?.instructions ?? '';
    const folder = input(r?.folder ?? '', 'Default: each bot\'s own folder');
    body.append(field('Name', name), field('Bots', members), field('Lead', lead, 'Answers when nobody is @mentioned, and can hand work to the others.'), field('Room instructions', instr), field('Shared folder', folder));
    dialog(r ? `Edit # ${r.name}` : 'New room', body, async () => {
      const ids = boxes.filter(([, c]) => c.checked).map(([id]) => id);
      if (!ids.includes(lead.value) && ids.length) ids.unshift(lead.value);
      const res = await window.void.roomsSave({ id: r?.id ?? '', name: name.value.trim(), members: ids, lead: lead.value, instructions: instr.value, folder: folder.value.trim() });
      await load();
      await openRoom(res.id);
    });
  }

  void load().then(() => {
    if (!target) drawWelcome();
  });
}
