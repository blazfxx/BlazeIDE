// BlBots tab: your roster of AI bots, their chats, rooms where bots work
// together, routines and each bot's private computer.
// All dynamic text is set with textContent (el() writes innerHTML).

import { el } from '../state';
import type { EditorArea } from '../editor/index';
import type { BotProfile, BotRoutine, BotsOverview } from '../../shared/ipc';

type Target = { kind: 'bot'; id: string; thread: string | null } | { kind: 'room'; id: string } | null;

function t(tag: keyof HTMLElementTagNameMap, cls: string, text?: string): HTMLElement {
  const e = el(tag, cls);
  if (text !== undefined) e.textContent = text;
  return e;
}

function avatar(b: { avatar: string; color: string; name: string }): HTMLElement {
  const a = t('span', 'bb-avatar', b.avatar || b.name.slice(0, 1));
  if (/^#[0-9a-fA-F]{6}$/.test(b.color)) a.style.background = b.color;
  return a;
}

export function openBlBots(editor: EditorArea): void {
  editor.openCustomTab('blbots', 'BlBots', (host) => render(host));
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

  function drawSide(): void {
    side.textContent = '';
    const head = el('div', 'bb-side-head');
    head.append(t('div', 'bb-title', 'BlBots'));
    const nb = t('button', 'primary', 'New bot') as HTMLButtonElement;
    nb.addEventListener('click', () => editBot(null));
    head.appendChild(nb);
    side.appendChild(head);
    if (!data.bots.length) side.appendChild(t('div', 'bb-empty', 'No bots yet. A bot is an AI with its own name, personality, model, folder and memory. Create one to start.'));
    for (const b of data.bots.filter((x) => !x.hidden)) {
      const row = el('button', 'bb-item');
      if (target?.kind === 'bot' && target.id === b.id) row.classList.add('active');
      const txt = el('span', 'bb-item-text');
      txt.append(t('span', 'bb-item-name', (b.pinned ? '📌 ' : '') + b.name), t('span', 'bb-item-sub', b.title || engineLabel(b)));
      row.append(avatar(b), txt);
      row.addEventListener('click', () => openBot(b.id, null));
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); botMenu(b, e.clientX, e.clientY); });
      side.appendChild(row);
    }
    const rh = el('div', 'bb-side-head bb-rooms-head');
    rh.append(t('div', 'bb-title', 'Rooms'));
    const nr = t('button', '', 'New room') as HTMLButtonElement;
    nr.disabled = data.bots.length === 0;
    nr.addEventListener('click', () => editRoom(null));
    rh.appendChild(nr);
    side.appendChild(rh);
    if (!data.rooms.length) side.appendChild(t('div', 'bb-empty', 'A room is one chat with several bots: @mention one, or let the lead hand out the work.'));
    for (const r of data.rooms) {
      const row = el('button', 'bb-item');
      if (target?.kind === 'room' && target.id === r.id) row.classList.add('active');
      const txt = el('span', 'bb-item-text');
      txt.append(t('span', 'bb-item-name', '# ' + r.name), t('span', 'bb-item-sub', `${r.members.length} bots`));
      row.appendChild(txt);
      row.addEventListener('click', () => void openRoom(r.id));
      row.addEventListener('contextmenu', (e) => { e.preventDefault(); roomMenu(r.id, e.clientX, e.clientY); });
      side.appendChild(row);
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
      ['Delete…', () => { if (confirm(`Delete ${b.name}, its chats and memory? Its working folder is kept.`)) void window.void.botsDelete(b.id).then(() => { target = null; main.textContent = ''; return load(); }); }],
    ]);
  }

  function roomMenu(id: string, x: number, y: number): void {
    const r = data.rooms.find((z) => z.id === id);
    menu(x, y, [
      ['Edit', () => r && editRoom(r)],
      ['Delete…', () => { if (confirm('Delete this room and its history?')) void window.void.roomsDelete(id).then(() => { target = null; main.textContent = ''; return load(); }); }],
    ]);
  }

  // ---------- conversation view ----------
  function frame(title: string, sub: string, actions: HTMLElement[]): HTMLElement {
    main.textContent = '';
    const head = el('div', 'bb-head');
    const h = el('div', 'bb-head-text');
    h.append(t('div', 'bb-head-title', title), t('div', 'bb-head-sub', sub));
    head.appendChild(h);
    for (const a of actions) head.appendChild(a);
    msgs = el('div', 'bb-msgs');
    const comp = el('div', 'bb-composer');
    const ta = el('textarea') as HTMLTextAreaElement;
    ta.rows = 2;
    ta.placeholder = target?.kind === 'room' ? 'Message the room (@Name to address a bot)' : 'Message';
    sendBtn = t('button', 'primary', 'Send') as HTMLButtonElement;
    const send = (): void => {
      if (runId) { void window.void.botsCancel(runId); return; }
      const text = ta.value.trim();
      if (!text || !target) return;
      ta.value = '';
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
    comp.append(ta, sendBtn);
    main.append(head, msgs, comp);
    return msgs;
  }

  function setWorking(on: boolean): void {
    if (sendBtn) sendBtn.textContent = on ? 'Stop' : 'Send';
    main.classList.toggle('bb-working', on);
  }

  function scrollDown(): void { if (msgs) msgs.scrollTop = msgs.scrollHeight; }

  function addMsg(m: { role: string; text: string; tools?: string[]; author?: string }): void {
    if (!msgs) return;
    const d = el('div', `bb-msg ${m.role}`);
    if (m.author) d.appendChild(t('div', 'bb-author', m.author));
    if (m.text?.trim()) d.appendChild(t('div', 'bb-text', m.text));
    msgs.appendChild(d);
    for (const tool of m.tools ?? []) msgs.appendChild(t('div', 'bb-tool', `● ${tool}`));
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
    const edit = t('button', '', 'Edit') as HTMLButtonElement;
    edit.addEventListener('click', () => editBot(b));
    const actions: HTMLElement[] = [sel, edit];
    if (b.computer) {
      const comp = t('button', '', 'Computer') as HTMLButtonElement;
      comp.addEventListener('click', () => {
        comp.disabled = true;
        comp.textContent = 'Starting…';
        void window.void.botsComputer(b.id).then((url) => openComputer(b.name, url)).catch((e: unknown) => addNote(String(e instanceof Error ? e.message : e), true)).finally(() => { comp.disabled = false; comp.textContent = 'Computer'; });
      });
      actions.push(comp);
    }
    frame(b.name, `${b.title ? b.title + ' · ' : ''}${engineLabel(b)} · ${b.approval === 'full' ? 'Full access' : b.approval === 'edits' ? 'Approve edits' : 'Asks first'}`, actions);
    if (thread) {
      chatId = thread;
      const th = await window.void.botsThread(id, thread);
      th.messages.forEach(addMsg);
      th.pending.forEach(addCard);
    } else {
      chatId = null;
      addNote(info.memory.trim() ? `${b.name} remembers:\n${info.memory.trim()}` : `Say hi to ${b.name}.`);
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
    const edit = t('button', '', 'Edit') as HTMLButtonElement;
    edit.addEventListener('click', () => room && editRoom(room));
    frame(`# ${r.name}`, names.join(', '), [edit]);
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
        liveEl = el('div', 'bb-msg assistant');
        if (target?.kind === 'room') liveEl.appendChild(t('div', 'bb-author', data.bots.find((b) => b.id === e.bot)?.name ?? 'bot'));
        const body = t('div', 'bb-text', '');
        liveEl.appendChild(body);
        msgs.appendChild(liveEl);
        liveText = '';
        liveBot = e.bot;
      }
      liveText += ev.text;
      const body = liveEl?.querySelector('.bb-text');
      if (body) body.textContent = liveText;
      scrollDown();
    } else if (ev.type === 'tool_start') {
      liveEl = null;
      msgs?.appendChild(t('div', 'bb-tool run', `● ${ev.summary}`));
      scrollDown();
    } else if (ev.type === 'tool_end' && ev.is_error) {
      addNote(`✗ ${String(ev.result).split('\n')[0].slice(0, 200)}`, true);
    } else if (ev.type === 'file_changed') {
      msgs?.appendChild(t('div', 'bb-tool', `${ev.created ? 'new' : 'edit'} ${ev.rel}`));
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
    cancel.addEventListener('click', () => ov.remove());
    save.addEventListener('click', () => {
      save.disabled = true;
      err.textContent = '';
      void onSave().then(() => ov.remove()).catch((e: unknown) => { err.textContent = String(e instanceof Error ? e.message : e); }).finally(() => { save.disabled = false; });
    });
    row.append(cancel, save);
    box.append(t('div', 'bb-head-title', title), body, err, row);
    ov.appendChild(box);
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

  function editBot(b: BotProfile | null): void {
    const body = el('div', 'bb-form');
    const name = input(b?.name ?? '', 'Nova');
    const av = input(b?.avatar ?? '', '🤖');
    av.maxLength = 4;
    const color = el('input') as HTMLInputElement;
    color.type = 'color';
    color.value = /^#[0-9a-fA-F]{6}$/.test(b?.color ?? '') ? b!.color : '#3a3a3a';
    const title = input(b?.title ?? '', 'Writes and fixes tests');
    const soul = el('textarea') as HTMLTextAreaElement;
    soul.rows = 6;
    soul.value = b?.soul ?? '';
    soul.placeholder = 'Who this bot is and how it works: tone, expertise, rules it always follows.';
    const engineOf = (k: string): string => data.engines.find((e) => e.name === k)?.path ? '' : ' (not found on this computer)';
    const engine = select([['blaze', 'BlazeAPI model'], ['claude', `Claude Code CLI${engineOf('claude')}`], ['codex', `Codex CLI${engineOf('codex')}`], ['grok', `Grok CLI${engineOf('grok')}`]], b?.engine.kind ?? 'blaze');
    const model = input(b?.model ?? '', 'Default: your selected model');
    const approval = select([['ask', 'Ask before changes and commands'], ['edits', 'Apply edits, ask before commands'], ['full', 'Full access (never asks)']], b?.approval ?? 'ask');
    const mode = select([['agent', 'Agent (uses tools)'], ['ask', 'Talk only']], b?.mode === 'ask' ? 'ask' : 'agent');
    const folder = input(b?.folder ?? '', 'Default: its own folder');
    const computer = el('input') as HTMLInputElement;
    computer.type = 'checkbox';
    computer.checked = !!b?.computer;
    const routines: BotRoutine[] = (b?.routines ?? []).map((r) => ({ ...r }));
    const rBox = el('div', 'bb-routines');
    const drawRoutines = (): void => {
      rBox.textContent = '';
      routines.forEach((r, i) => {
        const row = el('div', 'bb-routine');
        const rt = input(r.title, 'Morning report');
        const rp = input(r.prompt, 'What to do each time');
        const kind = select([['daily', 'Every day at'], ['weekdays', 'Weekdays at'], ['every', 'Every N hours'], ['cron', 'Cron']], r.schedule.kind === 'weekly' ? 'daily' : r.schedule.kind);
        const val = input(r.schedule.kind === 'every' ? String(r.schedule.hours) : r.schedule.kind === 'cron' ? r.schedule.expr : ('time' in r.schedule ? r.schedule.time : '09:00'));
        const on = el('input') as HTMLInputElement;
        on.type = 'checkbox';
        on.checked = r.enabled;
        on.title = 'On';
        const del = t('button', '', '✕');
        const sync = (): void => {
          r.title = rt.value; r.prompt = rp.value; r.enabled = on.checked;
          const k = kind.value;
          r.schedule = k === 'every' ? { kind: 'every', hours: Number(val.value) || 1 } : k === 'cron' ? { kind: 'cron', expr: val.value } : { kind: k as 'daily' | 'weekdays', time: val.value };
        };
        [rt, rp, kind, val, on].forEach((x) => x.addEventListener('change', sync));
        del.addEventListener('click', () => { routines.splice(i, 1); drawRoutines(); });
        row.append(on, rt, kind, val, del, rp);
        rBox.appendChild(row);
      });
      const add = t('button', '', 'Add routine');
      add.addEventListener('click', () => { routines.push({ id: `t${Math.random().toString(36).slice(2, 10)}`, title: '', prompt: '', schedule: { kind: 'daily', time: '09:00' }, enabled: true, catchUp: false, lastRun: 0, thread: null }); drawRoutines(); });
      rBox.appendChild(add);
    };
    drawRoutines();
    body.append(
      field('Name', name), field('Avatar', av, 'One emoji or letter'), field('Colour', color), field('Subtitle', title),
      field('Personality and instructions', soul, 'Sent with every message (up to 24 KB).'),
      field('Runs on', engine, 'Claude Code / Codex / Grok use the CLI installed on this computer and your own subscription.'),
      field('Model', model), field('What it may do', approval), field('Mode', mode),
      field('Working folder', folder, 'Its tools can only touch this folder.'),
      field('Private computer', computer, 'A Linux desktop in Docker it can see and use (needs Docker; about 4 GB RAM while running).'),
      field('Routines', rBox, 'Run while Blaze IDE (or `blaze serve`) is running.'),
    );
    dialog(b ? `Edit ${b.name}` : 'New bot', body, async () => {
      const k = engine.value;
      const profile: BotProfile = {
        ...(b ?? { id: `b${Math.random().toString(36).slice(2, 12)}`, checkBacks: [], pinned: false, hidden: false, created: Math.floor(Date.now() / 1000) }),
        name: name.value.trim(), avatar: av.value.trim() || name.value.trim().slice(0, 1).toUpperCase(), color: color.value, title: title.value.trim(), soul: soul.value,
        engine: k === 'blaze' ? { kind: 'blaze' } : { kind: k as 'claude' | 'codex' | 'grok', bin: null },
        model: model.value.trim(), approval: approval.value as BotProfile['approval'], mode: mode.value, folder: folder.value.trim(), computer: computer.checked, routines,
      } as BotProfile;
      const saved = await window.void.botsSave(profile);
      await load();
      await openBot(saved.id, null);
    });
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
    if (!data.bots.length) editBot(null);
  });
}
