// Making and editing a bot: one dialog with sections on the left (Identity,
// Soul, Brain, Memory, Routines, Access, Permissions) and Cancel / Create on
// the bottom. Identity has the avatar builder: picture upload or AI picture,
// frame, feature colour, expression, colour and body.
// All dynamic text goes in with textContent.

import { el } from '../state';
import type { BotProfile, BotRoutine, BotsOverview } from '../../shared/ipc';
import type { ModelEntry } from '../../shared/types';
import { openSkillsLibrary, selectBar, toggle } from '../skills/library';
import { avatarSvg, BODIES, FACE_COLORS, FACE_COUNT, FRAMES, type AvatarStyle } from './faces';

function t(tag: keyof HTMLElementTagNameMap, cls: string, text?: string): HTMLElement {
  const e = el(tag, cls);
  if (text !== undefined) e.textContent = text;
  return e;
}

export interface Template {
  key: string;
  label: string;
  blurb: string;
  bot: Partial<BotProfile>;
}

const rid = (): string => Math.random().toString(36).slice(2, 10);

export const TEMPLATES: Template[] = [
  {
    key: 'coder', label: 'Coder', blurb: 'Builds features and fixes bugs in your projects.',
    bot: { name: 'Nova', face: 1, body: 'bot', color: '#4a7fe0', title: 'Builds features and fixes bugs', approval: 'edits', mode: 'agent',
      soul: 'You are Nova, a careful software engineer.\nRead the relevant code before you change it. Keep changes small and focused. Run the tests after a change and fix what you broke.\nWhen you finish, say in a few lines what you changed and why.' },
  },
  {
    key: 'reviewer', label: 'Reviewer', blurb: 'Reads changes and points out bugs and risks.',
    bot: { name: 'Iris', face: 4, body: 'hex', color: '#7c5cc9', title: 'Reviews code and finds bugs', approval: 'ask', mode: 'agent',
      soul: 'You are Iris, a thorough code reviewer.\nLook for real bugs first: wrong logic, missing error handling, security problems, race conditions. Then readability.\nQuote the exact lines, explain the problem, and suggest a fix. Do not change files unless asked.' },
  },
  {
    key: 'researcher', label: 'Researcher', blurb: 'Digs into a question and writes a clear summary.',
    bot: { name: 'Sage', face: 2, body: 'blob', color: '#45a5c4', title: 'Researches and summarises', approval: 'ask', mode: 'agent',
      soul: 'You are Sage, a patient researcher.\nBreak a question into parts, check each one, and say how sure you are.\nAnswer with a short summary first, then the details. Keep notes in your folder so you can build on them later.' },
  },
  {
    key: 'writer', label: 'Writer', blurb: 'Writes docs, READMEs, release notes and posts.',
    bot: { name: 'Quill', face: 6, body: 'drop', color: '#e08a3e', title: 'Writes docs and release notes', approval: 'edits', mode: 'agent',
      soul: 'You are Quill, a clear and friendly technical writer.\nWrite short sentences in plain words. Lead with what the reader needs. Use examples.\nMatch the tone of what already exists in the project.' },
  },
  {
    key: 'reporter', label: 'Daily reporter', blurb: 'Sends you a summary of what changed, every morning.',
    bot: { name: 'Echo', face: 5, body: 'bot', color: '#d9ae3e', title: 'Morning summary of the project', approval: 'ask', mode: 'agent',
      soul: 'You are Echo. Each morning you look at what changed in the project (git log, open TODOs) and write a short summary: what happened, what is unfinished, and what to look at first.',
      routines: [{ id: `t${rid()}`, title: 'Morning summary', prompt: 'Summarise what changed in this project since yesterday and what to look at first.', schedule: { kind: 'weekdays', time: '09:00' }, enabled: true, catchUp: true, lastRun: 0, thread: null }] },
  },
];

const DEFAULT_SOUL = 'You are a capable, plain-spoken assistant.\nAsk one clarifying question when a request is ambiguous; otherwise do the work and show the result.\nLead with the answer, then the detail. Keep replies short and concrete.\nSay plainly when you are unsure or could not check something, and never present a guess as a fact.';

const VIBES: Array<[string, string]> = [
  ['Friendly', 'Be warm and encouraging.'],
  ['Straight to the point', 'Keep answers short. No filler.'],
  ['Explains its thinking', 'Explain your reasoning briefly before acting.'],
  ['Asks before guessing', 'If something is unclear, ask one short question instead of guessing.'],
  ['Funny', 'Light humour is welcome when it fits.'],
];

const ENGINES: Array<{ kind: 'blaze' | 'claude' | 'codex' | 'grok'; name: string; sub: string }> = [
  { kind: 'blaze', name: 'BlazeAPI', sub: 'Any BlazeAPI model, billed to your balance.' },
  { kind: 'claude', name: 'Claude Code', sub: 'Your Claude plan, through the claude CLI.' },
  { kind: 'codex', name: 'Codex', sub: 'Your ChatGPT plan, through the codex CLI.' },
  { kind: 'grok', name: 'Grok CLI', sub: 'Your Grok account, through the grok CLI.' },
];

const ACCESS: Array<{ v: BotProfile['approval']; name: string; sub: string; icon: string }> = [
  { v: 'ask', name: 'Ask first', sub: 'Asks before every edit and every command. Safest.', icon: '✋' },
  { v: 'edits', name: 'Edits are fine', sub: 'Changes files on its own, asks before running commands.', icon: '✎' },
  { v: 'auto', name: 'Auto', sub: 'Does everything on its own, except destructive actions: deleting, force-pushing, publishing, system changes, or clicking things like Buy or Delete on a website.', icon: '✦' },
  { v: 'full', name: 'YOLO', sub: 'Approves everything, including destructive commands. Only for folders you can afford to lose.', icon: '⚡' },
];

const SOUL_MAX = 24_000;

type Avatarish = { avatar: string; color: string; name: string; face?: number; body?: string; frame?: string; features?: string; image?: string };

/** The bot's avatar element: its picture, its drawn avatar, or its letter/emoji. */
export function botAvatar(b: Avatarish, size: number): HTMLElement {
  if (b.image && /^data:image\/(png|jpeg|webp|gif);base64,/.test(b.image)) {
    const s = el('span', `bb-avatar pic ${b.frame === 'circle' ? 'round' : b.frame === 'square' ? 'square' : ''}`);
    s.style.width = s.style.height = `${size}px`;
    const img = document.createElement('img');
    img.src = b.image;
    img.alt = '';
    s.appendChild(img);
    return s;
  }
  if ((b.face && b.face >= 1 && b.face <= FACE_COUNT) || b.body) {
    const s = el('span', 'bb-avatar face');
    s.style.width = s.style.height = `${size}px`;
    s.innerHTML = avatarSvg(b as AvatarStyle, size);
    return s;
  }
  const a = t('span', 'bb-avatar', b.avatar || b.name.slice(0, 1) || '?');
  a.style.width = a.style.height = `${size}px`;
  a.style.fontSize = `${Math.round(size * 0.42)}px`;
  if (/^#[0-9a-fA-F]{6}$/.test(b.color)) a.style.background = b.color;
  return a;
}

/** Read a picture file and scale it to fit 256x256 (keeps the saved bot small). */
function shrinkPicture(src: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const s = Math.min(1, 256 / Math.max(img.width, img.height));
      const c = document.createElement('canvas');
      c.width = Math.max(1, Math.round(img.width * s));
      c.height = Math.max(1, Math.round(img.height * s));
      c.getContext('2d')!.drawImage(img, 0, 0, c.width, c.height);
      let url = c.toDataURL('image/webp', 0.9);
      if (!url.startsWith('data:image/webp')) url = c.toDataURL('image/png');
      if (url.length > 290_000) url = c.toDataURL('image/jpeg', 0.82);
      resolve(url);
    };
    img.onerror = () => reject(new Error('That file is not a picture Blaze IDE can read'));
    img.src = src;
  });
}

export function openBotEditor(opts: {
  bot: BotProfile | null;
  template?: Template;
  engines: BotsOverview['engines'];
  memory?: string;
  onSave: (p: BotProfile, memory: string | null) => Promise<void>;
}): void {
  const isNew = !opts.bot;
  const seed: Partial<BotProfile> = opts.bot ?? opts.template?.bot ?? {};
  const st = {
    name: seed.name ?? '',
    title: seed.title ?? '',
    soul: seed.soul ?? (isNew ? DEFAULT_SOUL : ''),
    face: opts.bot ? (opts.bot.face ?? 0) : (seed.face ?? 1 + Math.floor(Math.random() * FACE_COUNT)),
    body: seed.body ?? (opts.bot && !opts.bot.face ? '' : 'bot'),
    frame: seed.frame ?? '',
    features: seed.features ?? '',
    image: seed.image ?? '',
    avatar: seed.avatar ?? '',
    color: seed.color && /^#[0-9a-fA-F]{6}$/.test(seed.color) && seed.color !== '#3a3a3a' ? seed.color : FACE_COLORS[Math.floor(Math.random() * 10)],
    engine: (seed.engine?.kind ?? 'blaze') as 'blaze' | 'claude' | 'codex' | 'grok',
    model: seed.model ?? '',
    approval: seed.approval ?? 'ask',
    mode: seed.mode === 'ask' ? 'ask' : 'agent',
    folder: seed.folder ?? '',
    computer: !!seed.computer,
    web: seed.web ?? true,
    skills: [...(seed.skills ?? [])] as string[],
    routines: ((seed.routines ?? []) as BotRoutine[]).map((r) => ({ ...r, schedule: { ...r.schedule } })) as BotRoutine[],
    memory: opts.memory ?? '',
    memoryTouched: false,
    role: opts.template?.key ?? 'custom',
  };
  const drawn = (): boolean => st.face > 0 || !!st.body;

  const ov = el('div', 'bb-overlay');
  const box = el('div', 'bbe');
  ov.appendChild(box);
  const head = el('div', 'bbe-head');
  const x = t('button', 'bbe-x', '✕') as HTMLButtonElement;
  x.title = 'Close';
  head.append(t('div', 'bbe-title', isNew ? 'New Bot' : `Edit ${opts.bot!.name}`), x);
  const cols = el('div', 'bbe-cols');
  const nav = el('div', 'bbe-nav');
  const body = el('div', 'bbe-body');
  cols.append(nav, body);
  const foot = el('div', 'bbe-foot');
  const err = t('div', 'bbe-err', '');
  const cancel = t('button', 'bbe-cancel', 'Cancel') as HTMLButtonElement;
  const save = t('button', 'bbe-save', isNew ? 'Create bot' : 'Save') as HTMLButtonElement;
  foot.append(err, cancel, save);
  box.append(head, cols, foot);

  const SECTIONS = ['Identity', 'Soul', 'Skills', 'Brain', 'Memory', 'Routines', 'Access', 'Permissions'];
  let section = 'Identity';

  // ---------- small builders ----------
  const card = (title?: string, sub?: string): HTMLElement => {
    const c = el('div', 'bbe-card');
    if (title) c.appendChild(t('div', 'bbe-card-title', title));
    if (sub) c.appendChild(t('div', 'bbe-card-sub', sub));
    body.appendChild(c);
    return c;
  };
  const caps = (text: string): HTMLElement => t('div', 'bbe-caps', text);
  function seg(options: Array<[string, string]>, value: string, on: (v: string) => void, cls = ''): HTMLElement {
    const s = el('div', `bbe-seg ${cls}`);
    for (const [v, l] of options) {
      const b = t('button', `bbe-seg-b${v === value ? ' on' : ''}`, l) as HTMLButtonElement;
      b.type = 'button';
      b.addEventListener('click', () => { on(v); draw(); });
      s.appendChild(b);
    }
    return s;
  }
  function textInput(value: string, ph: string, on: (v: string) => void, max = 200): HTMLInputElement {
    const i = el('input', 'bbe-input') as HTMLInputElement;
    i.value = value;
    i.placeholder = ph;
    i.maxLength = max;
    i.addEventListener('input', () => { on(i.value); syncFoot(); refreshPreview(); });
    return i;
  }
  const style = (over: Partial<AvatarStyle> = {}): AvatarStyle => ({ face: st.face || 1, body: st.body || 'bot', color: st.color, features: st.features, frame: st.frame, ...over });

  let previewEl: HTMLElement | null = null;
  function refreshPreview(): void {
    if (!previewEl) return;
    previewEl.textContent = '';
    previewEl.appendChild(botAvatar({ name: st.name || 'B', avatar: st.avatar, color: st.color, face: drawn() ? st.face || 1 : 0, body: drawn() ? st.body || 'bot' : '', frame: st.frame, features: st.features, image: st.image }, 150));
  }

  // ---------- sections ----------
  function identity(): void {
    const roleRow = el('div', 'bbe-field');
    roleRow.appendChild(t('div', 'bbe-label', 'Starting role'));
    const role = el('select', 'bbe-input big') as HTMLSelectElement;
    for (const [v, l] of [['custom', 'Custom settings'], ...TEMPLATES.map((tp) => [tp.key, `${tp.label}: ${tp.blurb}`])] as Array<[string, string]>) {
      const o = el('option') as HTMLOptionElement;
      o.value = v; o.textContent = l; role.appendChild(o);
    }
    role.value = st.role;
    role.addEventListener('change', () => {
      st.role = role.value;
      const tp = TEMPLATES.find((x) => x.key === role.value);
      if (tp) {
        Object.assign(st, {
          name: tp.bot.name ?? st.name, title: tp.bot.title ?? '', soul: tp.bot.soul ?? st.soul, face: tp.bot.face ?? st.face, body: tp.bot.body ?? 'bot',
          color: tp.bot.color ?? st.color, approval: tp.bot.approval ?? 'ask', mode: tp.bot.mode ?? 'agent', image: '',
          routines: (tp.bot.routines ?? []).map((r) => ({ ...r, id: `t${rid()}`, schedule: { ...r.schedule } })),
        });
      }
      draw();
    });
    roleRow.appendChild(role);
    body.appendChild(roleRow);

    const names = el('div', 'bbe-row2');
    const n = el('div', 'bbe-field');
    n.append(t('div', 'bbe-label', 'Name'), textInput(st.name, 'Nova', (v) => { st.name = v; }, 40));
    const s = el('div', 'bbe-field');
    s.append(t('div', 'bbe-label', 'What it helps with'), textInput(st.title, 'Builds features and fixes bugs', (v) => { st.title = v; }));
    names.append(n, s);
    body.appendChild(names);

    // Avatar card
    const av = el('div', 'bbe-avcard');
    const avHead = el('div', 'bbe-avhead');
    const reset = t('button', 'bbe-link', 'Reset avatar') as HTMLButtonElement;
    reset.type = 'button';
    reset.addEventListener('click', () => { Object.assign(st, { face: 1, body: 'bot', frame: '', features: '', image: '', avatar: '' }); draw(); });
    avHead.append(t('span', 'bbe-tab', 'Avatar'), reset);
    av.appendChild(avHead);
    const stage = el('div', 'bbe-stage');
    previewEl = el('div', 'bbe-preview');
    stage.appendChild(previewEl);
    av.appendChild(stage);
    refreshPreview();

    const inner = el('div', 'bbe-avbody');
    const file = el('input') as HTMLInputElement;
    file.type = 'file';
    file.accept = 'image/png,image/jpeg,image/gif,image/webp';
    file.style.display = 'none';
    const up = el('button', 'bbe-upload') as HTMLButtonElement;
    up.type = 'button';
    up.append(t('span', 'bbe-upload-ic', '⬆'), t('span', '', st.image ? 'Replace picture' : 'Upload image'));
    up.addEventListener('click', () => file.click());
    file.addEventListener('change', () => {
      const f = file.files?.[0];
      if (!f) return;
      if (f.size > 10 * 1024 * 1024) { err.textContent = 'That picture is over 10 MB.'; return; }
      const r = new FileReader();
      r.onload = () => void shrinkPicture(String(r.result)).then((u) => { st.image = u; draw(); }, (e: Error) => { err.textContent = e.message; });
      r.readAsDataURL(f);
    });
    inner.append(up, file);
    const note = el('div', 'bbe-note');
    note.appendChild(t('span', '', 'PNG, JPEG, GIF or WebP · up to 10 MB'));
    if (st.image) {
      const rm = t('button', 'bbe-link', 'Use the drawn avatar instead') as HTMLButtonElement;
      rm.type = 'button';
      rm.addEventListener('click', () => { st.image = ''; draw(); });
      note.appendChild(rm);
    }
    inner.appendChild(note);

    inner.appendChild(caps('Frame'));
    inner.appendChild(seg(FRAMES.map((f) => [f.key, f.key ? f.label : 'None']), st.frame, (v) => { st.frame = v; }));
    const fh = el('div', 'bbe-split');
    fh.append(t('span', 'bbe-label plain', 'Facial features'), t('span', 'bbe-hint', st.features === 'black' ? 'Black' : 'White'));
    inner.appendChild(fh);
    inner.appendChild(seg([['', 'White'], ['black', 'Black']], st.features === 'black' ? 'black' : '', (v) => { st.features = v; }, 'round'));

    inner.appendChild(caps('Expression'));
    const ex = el('div', 'bbe-grid five');
    for (let f = 1; f <= FACE_COUNT; f++) {
      const b = el('button', `bbe-tile${drawn() && (st.face || 1) === f && !st.image ? ' on' : ''}`) as HTMLButtonElement;
      b.type = 'button';
      b.title = `Expression ${f}`;
      b.innerHTML = avatarSvg(style({ face: f, frame: '' }), 46);
      b.addEventListener('click', () => { st.face = f; if (!st.body) st.body = 'bot'; st.image = ''; draw(); });
      ex.appendChild(b);
    }
    inner.appendChild(ex);

    inner.appendChild(caps('Color'));
    const cs = el('div', 'bbe-colors');
    for (const c of FACE_COLORS) {
      const b = el('button', `bbe-color${st.color === c ? ' on' : ''}`) as HTMLButtonElement;
      b.type = 'button';
      b.style.background = `radial-gradient(circle at 35% 30%, color-mix(in srgb, ${c} 55%, #fff), ${c} 60%)`;
      b.title = c;
      b.addEventListener('click', () => { st.color = c; draw(); });
      cs.appendChild(b);
    }
    inner.appendChild(cs);

    inner.appendChild(caps('Body'));
    const bg = el('div', 'bbe-grid five');
    for (const bd of BODIES) {
      const b = el('button', `bbe-tile${drawn() && (st.body || 'bot') === bd.key && !st.image ? ' on' : ''}`) as HTMLButtonElement;
      b.type = 'button';
      b.title = bd.label;
      b.innerHTML = avatarSvg(style({ body: bd.key, frame: '' }), 42);
      b.addEventListener('click', () => { st.body = bd.key; if (!st.face) st.face = 1; st.image = ''; draw(); });
      bg.appendChild(b);
    }
    inner.appendChild(bg);

    const letter = el('div', 'bbe-split');
    letter.appendChild(t('span', 'bbe-label plain', 'Or an emoji / letter'));
    const li = textInput(st.avatar, '🦊', (v) => { st.avatar = v.trim(); }, 4);
    li.classList.add('tiny');
    const useL = t('button', 'bbe-link', 'Use it') as HTMLButtonElement;
    useL.type = 'button';
    useL.addEventListener('click', () => { st.face = 0; st.body = ''; st.image = ''; draw(); });
    const lw = el('span', 'bbe-inline');
    lw.append(li, useL);
    letter.appendChild(lw);
    inner.appendChild(letter);

    const gen = el('div', 'bbe-gen');
    gen.appendChild(t('div', 'bbe-gen-title', '✦ Generate with AI'));
    gen.appendChild(t('div', 'bbe-hint', 'Draws a picture with your BlazeAPI image model (one image from your balance).'));
    const prompt = el('textarea', 'bbe-input') as HTMLTextAreaElement;
    prompt.rows = 2;
    prompt.placeholder = `Optional direction, e.g. “a calm navigator${st.title ? `, ${st.title.toLowerCase()}` : ''}”`;
    const go = t('button', 'bbe-btn', 'Generate') as HTMLButtonElement;
    go.type = 'button';
    go.addEventListener('click', () => {
      go.disabled = true;
      go.textContent = 'Drawing…';
      err.textContent = '';
      const dir = [prompt.value.trim(), st.name && `named ${st.name}`, st.title].filter(Boolean).join(', ');
      void window.void.botsAvatarImage(dir)
        .then((u) => shrinkPicture(u))
        .then((u) => { st.image = u; draw(); })
        .catch((e: unknown) => { err.textContent = String(e instanceof Error ? e.message : e); go.disabled = false; go.textContent = 'Generate'; });
    });
    gen.append(prompt, go);
    inner.appendChild(gen);
    av.appendChild(inner);
    body.appendChild(av);
  }

  function soul(): void {
    body.appendChild(t('div', 'bbe-lead', 'Who this bot is and the rules it never breaks. Always in its context.'));
    const f = el('div', 'bbe-field');
    f.appendChild(t('div', 'bbe-label', 'Standing instructions'));
    const ta = el('textarea', 'bbe-input mono') as HTMLTextAreaElement;
    ta.rows = 12;
    ta.value = st.soul;
    const meta = el('div', 'bbe-split small');
    const count = t('span', 'bbe-hint', '');
    const upd = (): void => { count.textContent = `${new TextEncoder().encode(st.soul).length.toLocaleString()} / ${SOUL_MAX.toLocaleString()} bytes`; };
    ta.addEventListener('input', () => { st.soul = ta.value; upd(); });
    upd();
    meta.append(t('span', 'bbe-hint', 'Sent with every message.'), count);
    f.append(ta, meta);
    body.appendChild(f);
    const c = card('Quick additions', 'Adds a line to the instructions; click again to take it out.');
    const vibes = el('div', 'bbe-chips');
    for (const [name, line] of VIBES) {
      const on = st.soul.includes(line);
      const b = t('button', `bbe-chip${on ? ' on' : ''}`, `${on ? '✓' : '+'} ${name}`) as HTMLButtonElement;
      b.type = 'button';
      b.addEventListener('click', () => {
        st.soul = on ? st.soul.replace(`\n${line}`, '').replace(line, '').trim() : `${st.soul.trim()}${st.soul.trim() ? '\n' : ''}${line}`;
        draw();
      });
      vibes.appendChild(b);
    }
    c.appendChild(vibes);
  }

  let skillQuery = '';
  async function skills(): Promise<void> {
    body.appendChild(t('div', 'bbe-lead', 'Instructions for specific jobs. The bot sees each skill’s one-line description and opens the full skill when a task calls for it.'));
    const bar = el('div', 'bbe-inline grow');
    const search = el('input', 'bbe-input') as HTMLInputElement;
    search.placeholder = 'Search skills';
    search.value = skillQuery;
    const manage = t('button', 'bbe-btn', 'Library: add or import…') as HTMLButtonElement;
    manage.type = 'button';
    manage.addEventListener('click', () => openSkillsLibrary({ onClose: () => { if (ov.isConnected) draw(); } }));
    bar.append(search, manage);
    const list = el('div', 'bbe-skills');
    body.append(bar, list);
    const all = await window.void.skillsList().catch(() => []);
    if (!list.isConnected) return;
    const fill = (): void => {
      list.textContent = '';
      if (!all.length) {
        list.appendChild(t('div', 'bbe-card-sub', 'Your library is empty. Open the library to import your Claude Code, Codex or OpenCode skills, add a starter, or write one.'));
        return;
      }
      const shown = all.filter((x) => `${x.name} ${x.description}`.toLowerCase().includes(skillQuery.toLowerCase()));
      list.appendChild(selectBar(shown.length, shown.filter((x) => st.skills.includes(x.name)).length, all.filter((x) => st.skills.includes(x.name)).length, (every) => {
        const names = shown.map((x) => x.name);
        st.skills = every ? [...new Set([...st.skills, ...names])] : st.skills.filter((n) => !names.includes(n));
        fill();
      }));
      for (const s of shown) {
        const on = st.skills.includes(s.name);
        const r = el('div', `skl-row pick${on ? ' on' : ''}`);
        const mid = el('div', 'skl-mid');
        const top = el('div', 'skl-top');
        top.append(t('span', 'skl-name', s.name), t('span', 'skl-badge', s.source));
        mid.append(top, t('div', 'skl-desc', s.description));
        const sw = toggle(on, `Give ${s.name} to this bot`, () => {});
        r.append(mid, sw);
        r.addEventListener('click', () => {
          st.skills = on ? st.skills.filter((n) => n !== s.name) : [...st.skills, s.name];
          fill();
        });
        list.appendChild(r);
      }
    };
    search.addEventListener('input', () => { skillQuery = search.value; fill(); });
    fill();
  }

  async function brain(): Promise<void> {
    const c = card('Runs on', 'Which AI does the thinking.');
    const grid = el('div', 'bbe-options two');
    for (const e of ENGINES) {
      const found = e.kind === 'blaze' || !!opts.engines.find((x) => x.name === e.kind)?.path;
      const b = el('button', `bbe-option${st.engine === e.kind ? ' on' : ''}`) as HTMLButtonElement;
      b.type = 'button';
      b.append(t('div', 'bbe-option-name', e.name), t('div', 'bbe-option-sub', e.sub));
      if (e.kind !== 'blaze') b.appendChild(t('div', `bbe-badge${found ? ' ok' : ''}`, found ? 'Installed' : 'Not found on this computer'));
      b.addEventListener('click', () => { st.engine = e.kind; draw(); });
      grid.appendChild(b);
    }
    c.appendChild(grid);
    if (st.engine === 'blaze') {
      const m = card('Model');
      const sel = el('select', 'bbe-input') as HTMLSelectElement;
      const def = el('option') as HTMLOptionElement;
      def.value = '';
      def.textContent = 'Same as the IDE (your selected model)';
      sel.appendChild(def);
      sel.addEventListener('change', () => { st.model = sel.value; });
      m.appendChild(sel);
      const models: ModelEntry[] = await window.void.aiGetModels().catch(() => []);
      for (const mo of models.filter((x) => !x.kind || x.kind === 'chat')) {
        const o = el('option') as HTMLOptionElement;
        o.value = mo.id; o.textContent = mo.displayName || mo.id; sel.appendChild(o);
      }
      if (st.model && !models.some((mo) => mo.id === st.model)) {
        const o = el('option') as HTMLOptionElement;
        o.value = st.model; o.textContent = st.model; sel.appendChild(o);
      }
      sel.value = st.model;
    }
    const w = card('How it works');
    w.appendChild(seg([['agent', 'Does the work (uses tools)'], ['ask', 'Talk only']], st.mode, (v) => { st.mode = v; }));
  }

  function memory(): void {
    body.appendChild(t('div', 'bbe-lead', 'What this bot remembers between chats. It adds to this on its own; you can edit it any time.'));
    const f = el('div', 'bbe-field');
    f.appendChild(t('div', 'bbe-label', 'Memory'));
    const ta = el('textarea', 'bbe-input mono') as HTMLTextAreaElement;
    ta.rows = 12;
    ta.value = st.memory;
    ta.placeholder = '- I prefer TypeScript\n- The main project is ~/code/shop';
    ta.addEventListener('input', () => { st.memory = ta.value; st.memoryTouched = true; });
    f.appendChild(ta);
    body.appendChild(f);
  }

  function routines(): void {
    body.appendChild(t('div', 'bbe-lead', 'Things it does on a schedule, while Blaze IDE (or blaze serve) is open. Results land in a chat.'));
    st.routines.forEach((r, i) => {
      const c = el('div', 'bbe-card');
      const top = el('div', 'bbe-routine-top');
      const on = el('span', `switch${r.enabled ? ' on' : ''}`);
      on.appendChild(el('span', 'knob'));
      on.addEventListener('click', () => { r.enabled = !r.enabled; draw(); });
      const title = textInput(r.title, 'Morning report', (v) => { r.title = v; });
      const kind = el('select', 'bbe-input') as HTMLSelectElement;
      for (const [v, l] of [['daily', 'Every day at'], ['weekdays', 'Weekdays at'], ['every', 'Every N hours'], ['cron', 'Cron']]) {
        const o = el('option') as HTMLOptionElement;
        o.value = v; o.textContent = l; kind.appendChild(o);
      }
      kind.value = r.schedule.kind === 'weekly' ? 'daily' : r.schedule.kind;
      const val = textInput(r.schedule.kind === 'every' ? String(r.schedule.hours) : r.schedule.kind === 'cron' ? r.schedule.expr : ('time' in r.schedule ? r.schedule.time : '09:00'), '09:00', () => sync());
      const sync = (): void => {
        const k = kind.value;
        r.schedule = k === 'every' ? { kind: 'every', hours: Number(val.value) || 1 } : k === 'cron' ? { kind: 'cron', expr: val.value } : { kind: k as 'daily' | 'weekdays', time: val.value };
      };
      kind.addEventListener('change', sync);
      const del = t('button', 'bbe-x', '✕') as HTMLButtonElement;
      del.type = 'button';
      del.title = 'Remove';
      del.addEventListener('click', () => { st.routines.splice(i, 1); draw(); });
      top.append(on, title, kind, val, del);
      c.append(top, textInput(r.prompt, 'What to do each time', (v) => { r.prompt = v; }, 4000));
      body.appendChild(c);
    });
    const add = t('button', 'bbe-btn', '+ Add a routine') as HTMLButtonElement;
    add.type = 'button';
    add.addEventListener('click', () => { st.routines.push({ id: `t${rid()}`, title: '', prompt: '', schedule: { kind: 'daily', time: '09:00' }, enabled: true, catchUp: false, lastRun: 0, thread: null }); draw(); });
    body.appendChild(add);
  }

  function access(): void {
    const c = card('Working folder', 'Where this bot runs its shell and file tools. It cannot touch anything outside it.');
    const row = el('div', 'bbe-inline grow');
    const f = textInput(st.folder, 'Private bot folder, or an absolute path', (v) => { st.folder = v.trim(); }, 1000);
    f.classList.add('mono');
    const pick = t('button', 'bbe-btn', 'Choose…') as HTMLButtonElement;
    pick.type = 'button';
    pick.addEventListener('click', () => void window.void.fsOpenFolder().then((p) => { if (p) { st.folder = p; f.value = p; } }));
    row.append(f, pick);
    c.appendChild(row);
    const net = card();
    const netLine = el('div', 'bbe-split');
    const netText = el('div', '');
    netText.append(t('div', 'bbe-card-title', 'Web browsing'), t('div', 'bbe-card-sub', 'Search the web, read pages and use a hidden browser (your Chrome or Edge, with none of your logins). Public websites only.'));
    const netSwitch = toggle(st.web, 'Web browsing', (v) => { st.web = v; draw(); });
    netLine.append(netText, netSwitch);
    net.appendChild(netLine);
    const comp = card();
    const line = el('div', 'bbe-split');
    const tx = el('div', '');
    tx.append(t('div', 'bbe-card-title', 'Private computer'), t('div', 'bbe-card-sub', 'A Linux desktop in Docker that it can see and use. Needs Docker; about 4 GB of memory while it runs.'));
    const sw = el('span', `switch${st.computer ? ' on' : ''}`);
    sw.appendChild(el('span', 'knob'));
    sw.addEventListener('click', () => { st.computer = !st.computer; draw(); });
    line.append(tx, sw);
    comp.appendChild(line);
  }

  function permissions(): void {
    const c = card('Approval level', 'Default for this bot’s chats, routines and work handed to it in rooms.');
    for (const a of ACCESS) {
      const r = el('button', `bbe-radio${st.approval === a.v ? ' on' : ''}`) as HTMLButtonElement;
      r.type = 'button';
      const dot = el('span', 'bbe-dot');
      const tx = el('span', 'bbe-radio-text');
      tx.append(t('span', 'bbe-radio-name', `${a.icon}  ${a.name}`), t('span', 'bbe-card-sub', a.sub));
      r.append(dot, tx);
      r.addEventListener('click', () => { st.approval = a.v; draw(); });
      c.appendChild(r);
    }
  }

  // ---------- frame ----------
  function syncFoot(): void {
    save.disabled = !st.name.trim();
  }

  function draw(): void {
    const keep = body.scrollTop;
    nav.textContent = '';
    for (const s of SECTIONS) {
      const b = t('button', `bbe-nav-b${s === section ? ' on' : ''}`, s) as HTMLButtonElement;
      b.type = 'button';
      b.addEventListener('click', () => { section = s; draw(); body.scrollTop = 0; });
      nav.appendChild(b);
    }
    body.textContent = '';
    previewEl = null;
    if (section === 'Identity') identity();
    else if (section === 'Soul') soul();
    else if (section === 'Skills') void skills();
    else if (section === 'Brain') void brain();
    else if (section === 'Memory') memory();
    else if (section === 'Routines') routines();
    else if (section === 'Access') access();
    else permissions();
    body.scrollTop = keep;
    syncFoot();
  }

  const close = (): void => { ov.remove(); document.removeEventListener('keydown', onKey, true); };
  const onKey = (e: KeyboardEvent): void => { if (e.key === 'Escape' && !document.querySelector('.skl')) close(); };
  x.addEventListener('click', close);
  cancel.addEventListener('click', close);
  save.addEventListener('click', () => {
    const name = st.name.trim();
    if (!name) { section = 'Identity'; draw(); return; }
    save.disabled = true;
    err.textContent = '';
    const useDrawn = drawn();
    const profile: BotProfile = {
      ...(opts.bot ?? { id: `b${Math.random().toString(36).slice(2, 12)}`, checkBacks: [], pinned: false, hidden: false, created: Math.floor(Date.now() / 1000) }),
      name, title: st.title.trim(), soul: st.soul, color: st.color,
      face: useDrawn ? st.face || 1 : 0, body: useDrawn ? st.body || 'bot' : '', frame: st.frame, features: st.features, image: st.image,
      avatar: (st.avatar || name.slice(0, 1).toUpperCase()).slice(0, 4),
      engine: st.engine === 'blaze' ? { kind: 'blaze' } : { kind: st.engine, bin: opts.bot && opts.bot.engine.kind === st.engine && 'bin' in opts.bot.engine ? opts.bot.engine.bin : null },
      model: st.engine === 'blaze' ? st.model : '', approval: st.approval, mode: st.mode, folder: st.folder, computer: st.computer, routines: st.routines, skills: st.skills, web: st.web,
    } as BotProfile;
    void opts.onSave(profile, isNew ? (st.memory.trim() ? st.memory : null) : (st.memoryTouched ? st.memory : null))
      .then(close)
      .catch((e: unknown) => { err.textContent = String(e instanceof Error ? e.message : e); save.disabled = false; });
  });
  document.addEventListener('keydown', onKey, true);
  ov.addEventListener('mousedown', (e) => { if (e.target === ov) close(); });
  document.body.appendChild(ov);
  draw();
  setTimeout(() => (body.querySelector('input.bbe-input') as HTMLInputElement | null)?.focus(), 30);
}
