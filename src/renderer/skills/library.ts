// The skills window: your library, importing Claude Code / Codex / OpenCode
// skills from this computer, starter skills, and a SKILL.md editor.
// Skill names and text come from files: only textContent, never innerHTML.

import { el, state } from '../state';
import type { FoundSkill, SkillInfo } from '../../shared/ipc';

function t(tag: keyof HTMLElementTagNameMap, cls: string, text?: string): HTMLElement {
  const e = el(tag, cls);
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(cls: string, text: string, on: () => void): HTMLButtonElement {
  const b = t('button', cls, text) as HTMLButtonElement;
  b.type = 'button';
  b.addEventListener('click', on);
  return b;
}

export function toggle(on: boolean, label: string, change: (v: boolean) => void): HTMLButtonElement {
  const b = el('button', `switch${on ? ' on' : ''}`) as HTMLButtonElement;
  b.type = 'button';
  b.setAttribute('role', 'switch');
  b.setAttribute('aria-checked', String(on));
  b.setAttribute('aria-label', label);
  b.appendChild(el('span', 'knob'));
  b.addEventListener('click', () => change(!on));
  return b;
}

const NEW_SKILL = `---
name: my-skill
description: "What it does. Use when the user asks for ... (the AI picks skills by this line)"
---

# My skill

1. First step.
2. Second step.
`;

/** The bar above a list of skills: how many are picked, and one button that
 *  picks every skill shown (or none, when they all are). */
export function selectBar(shown: number, shownPicked: number, totalPicked: number, change: (all: boolean) => void): HTMLElement {
  const bar = el('div', 'skl-selectbar');
  const all = shown > 0 && shownPicked === shown;
  const b = button('bbe-btn', all ? 'Select none' : `Select all (${shown})`, () => change(!all));
  b.disabled = !shown;
  bar.append(t('span', 'skl-count', `${totalPicked} selected`), b);
  return bar;
}

/** About how many tokens the list of switched-on skills adds to each message. */
function tokenEstimate(skills: SkillInfo[]): number {
  const chars = skills.reduce((n, s) => n + s.name.length + Math.min(s.description.length, 240) + 4, 0);
  return chars ? Math.round((Math.min(chars, 15_000) + 300) / 4) : 0;
}

const errorText = (e: unknown): string => String(e instanceof Error ? e.message : e);

type Tab = 'mine' | 'import' | 'starters';

export function openSkillsLibrary(opts: { tab?: Tab; onClose?: () => void } = {}): void {
  if (document.querySelector('.skl')) return;
  const ov = el('div', 'bb-overlay skl-overlay');
  const box = el('div', 'bbe skl');
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-label', 'Skills library');
  ov.appendChild(box);

  let tab: Tab = opts.tab ?? 'mine';
  let editing: { old: string | null; text: string; files: string[] } | null = null;
  let found: FoundSkill[] | null = null;
  let fromFolder = false;
  let replace = false;
  const picked = new Set<string>();
  let query = '';
  let status = '';
  let bad = false;
  let confirmDelete = '';
  let seq = 0;

  const head = el('div', 'bbe-head');
  head.append(t('div', 'bbe-title', 'Skills'), button('bbe-x', '✕', () => close()));
  const bar = el('div', 'skl-bar');
  const body = el('div', 'bbe-body skl-body');
  const foot = el('div', 'bbe-foot skl-foot');
  box.append(head, bar, body, foot);

  const say = (msg: string, isError = false): void => { status = msg; bad = isError; };
  const run = (p: Promise<unknown>): void => { void p.catch((e: unknown) => { say(errorText(e), true); void draw(); }); };
  const matches = (s: { name: string; description: string; source?: string }): boolean => !query || `${s.name} ${s.description} ${s.source ?? ''}`.toLowerCase().includes(query.toLowerCase());

  function searchBox(): HTMLInputElement {
    const i = el('input', 'bbe-input skl-search') as HTMLInputElement;
    i.placeholder = 'Search skills';
    i.value = query;
    i.addEventListener('input', () => { query = i.value; run(draw()); });
    return i;
  }

  function row(name: string, description: string, badges: Array<[string, string]>, lead: HTMLElement | null, actions: HTMLElement[]): HTMLElement {
    const r = el('div', 'skl-row');
    if (lead) r.appendChild(lead);
    const mid = el('div', 'skl-mid');
    const top = el('div', 'skl-top');
    top.appendChild(t('span', 'skl-name', name));
    for (const [text, cls] of badges) top.appendChild(t('span', `skl-badge ${cls}`, text));
    mid.append(top, t('div', 'skl-desc', description));
    const act = el('div', 'skl-actions');
    act.append(...actions);
    r.append(mid, act);
    return r;
  }

  function empty(title: string, sub: string, ...actions: HTMLElement[]): HTMLElement {
    const e = el('div', 'skl-empty');
    e.append(t('div', 'skl-empty-title', title), t('div', 'bbe-card-sub', sub));
    if (actions.length) {
      const a = el('div', 'skl-empty-actions');
      a.append(...actions);
      e.appendChild(a);
    }
    return e;
  }

  async function edit(name: string): Promise<void> {
    const s = await window.void.skillsGet(name);
    editing = { old: name, text: s.text, files: s.files.filter((f) => f !== 'SKILL.md') };
    say('');
    await draw();
  }

  function drawEditor(): void {
    const e = editing!;
    body.appendChild(t('div', 'bbe-lead', 'A skill is a SKILL.md file: the two lines between --- name it and say when to use it; the rest is what the AI should do. Same format as Claude Code, Codex and OpenCode.'));
    const ta = el('textarea', 'bbe-input mono skl-editor') as HTMLTextAreaElement;
    ta.value = e.text;
    ta.spellcheck = false;
    ta.setAttribute('aria-label', 'SKILL.md');
    ta.addEventListener('input', () => { e.text = ta.value; });
    body.appendChild(ta);
    if (e.files.length) body.appendChild(t('div', 'bbe-hint', `Also in this skill: ${e.files.slice(0, 20).join(', ')}${e.files.length > 20 ? ` and ${e.files.length - 20} more` : ''}`));
    const save = button('bbe-save', e.old ? 'Save' : 'Add skill', () => {
      run(window.void.skillsSave(e.old, e.text).then(async (name) => { editing = null; tab = 'mine'; query = ''; say(`Saved ${name}.`); await draw(); }));
    });
    foot.append(t('div', `skl-status${bad ? ' bad' : ''}`, status), button('bbe-cancel', 'Back', () => { editing = null; say(''); run(draw()); }), save);
  }

  async function drawMine(mine: SkillInfo[]): Promise<void> {
    if (!mine.length) {
      body.appendChild(empty('No skills yet', 'Skills teach the AI how you want specific jobs done. Bring the ones you already have, start from a ready-made one, or write your own.',
        button('bbe-save', 'Import from this computer', () => { tab = 'import'; scan(); }),
        button('bbe-cancel', 'Browse starters', () => { tab = 'starters'; run(draw()); })));
      return;
    }
    for (const s of mine.filter(matches)) {
      const del = button(`bbe-btn${confirmDelete === s.name ? ' danger' : ''}`, confirmDelete === s.name ? 'Click again to delete' : 'Delete', () => {
        if (confirmDelete !== s.name) { confirmDelete = s.name; run(draw()); return; }
        confirmDelete = '';
        run(window.void.skillsRemove(s.name).then(() => { say(`Deleted ${s.name}.`); return draw(); }));
      });
      const inChat = el('label', 'skl-chat');
      inChat.append(t('span', '', 'In chat'), toggle(s.chat, `Use ${s.name} in chat`, (v) => run(window.void.skillsSetChat([s.name], v).then(() => draw()))));
      body.appendChild(row(s.name, s.description, [[s.source, ''], ...(s.files > 1 ? [[`${s.files} files`, ''] as [string, string]] : [])], null, [inChat, button('bbe-btn', 'Edit', () => run(edit(s.name))), del]));
    }
    const on = mine.filter((s) => s.chat);
    const all = button('bbe-btn', on.length ? 'Turn all off' : 'Turn all on', () => run(window.void.skillsSetChat(mine.map((s) => s.name), !on.length).then(() => draw())));
    const summary = on.length
      ? `${on.length} of ${mine.length} in chat · adds about ${tokenEstimate(on).toLocaleString()} tokens to each message. Bots choose their own.`
      : 'None are used in chat. Bots choose their own.';
    foot.append(t('div', `skl-status${bad ? ' bad' : ''}`, status || summary), all);
  }

  function drawImport(): void {
    if (!found) {
      body.appendChild(t('div', 'skl-empty', 'Looking on this computer…'));
      return;
    }
    const fresh = found.filter((f) => f.status !== 'same');
    if (!found.length) {
      body.appendChild(empty(fromFolder ? 'No SKILL.md in that folder' : 'No skills found on this computer',
        'Looked for Claude Code, Codex, OpenCode and ~/.agents skills and slash commands (and in the open project). You can also pick any folder.',
        button('bbe-save', 'Choose a folder…', pickFolder)));
      return;
    }
    const choosable = fresh.filter(matches);
    body.appendChild(selectBar(choosable.length, choosable.filter((f) => picked.has(f.path)).length, picked.size, (all) => {
      for (const f of choosable) if (all) picked.add(f.path); else picked.delete(f.path);
      run(draw());
    }));
    let group = '';
    for (const f of [...found].sort((a, b) => a.source.localeCompare(b.source) || a.name.localeCompare(b.name)).filter(matches)) {
      if (f.source !== group) {
        group = f.source;
        body.appendChild(t('div', 'bbe-caps skl-group', group));
      }
      const box_ = el('input', 'skl-check') as HTMLInputElement;
      box_.type = 'checkbox';
      box_.checked = picked.has(f.path);
      box_.disabled = f.status === 'same';
      box_.setAttribute('aria-label', `Import ${f.name}`);
      box_.addEventListener('change', () => { if (box_.checked) picked.add(f.path); else picked.delete(f.path); run(draw()); });
      const badges: Array<[string, string]> = f.status === 'same' ? [['In your library', 'ok']] : f.status === 'differs' ? [['Different version in library', 'warn']] : [];
      const r = row(f.name, f.description, badges, box_, []);
      if (f.status !== 'same') r.className = `skl-row pick${box_.checked ? ' on' : ''}`;
      r.title = f.path;
      r.addEventListener('click', (ev) => { if (ev.target !== box_ && !box_.disabled) box_.click(); });
      body.appendChild(r);
    }
    const left = el('div', 'skl-foot-left');
    left.appendChild(t('div', `skl-status${bad ? ' bad' : ''}`, status || `${found.length} found. Importing copies them; the originals stay as they are. Only import skills you trust: the AI follows them.`));
    if (found.some((f) => f.status === 'differs')) {
      const lab = el('label', 'skl-chat');
      lab.append(toggle(replace, 'Replace different versions', (v) => { replace = v; run(draw()); }), t('span', '', 'Replace different versions (otherwise they are added as “name-2”)'));
      left.appendChild(lab);
    }
    const go = button('bbe-save', picked.size ? `Import ${picked.size}` : 'Import', () => {
      go.disabled = true;
      run(window.void.skillsImport([...picked], replace).then(async (added) => {
        picked.clear();
        found = null;
        tab = 'mine';
        query = '';
        say(added.length ? `Imported ${added.length} skill${added.length === 1 ? '' : 's'}.` : 'Nothing new to import.');
        await draw();
      }));
    });
    go.disabled = !picked.size;
    foot.append(left, go);
  }

  async function drawStarters(): Promise<void> {
    const mySeq = seq;
    const list = await window.void.skillsStarters();
    if (mySeq !== seq) return;
    for (const s of list.filter(matches)) {
      const add = button('bbe-btn', s.added ? 'Added' : 'Add', () => run(window.void.skillsAddStarter(s.name).then(() => { say(`Added ${s.name}.`); return draw(); })));
      add.disabled = s.added;
      body.appendChild(row(s.name, s.description, [], null, [add]));
    }
    foot.append(t('div', `skl-status${bad ? ' bad' : ''}`, status || 'Ready-made skills that come with Blaze. Add one and edit it to suit you.'));
  }

  function scan(): void {
    fromFolder = false;
    found = null;
    picked.clear();
    run(draw().then(async () => {
      found = await window.void.skillsDiscover(state.projectRoot ?? null);
      await draw();
    }));
  }

  function pickFolder(): void {
    run(window.void.skillsPickFolder().then(async (list) => {
      if (!list) return;
      tab = 'import';
      fromFolder = true;
      found = list;
      picked.clear();
      for (const f of list) if (f.status !== 'same') picked.add(f.path);
      say('');
      await draw();
    }));
  }

  async function draw(): Promise<void> {
    const my = ++seq;
    const mine = await window.void.skillsList();
    if (my !== seq) return;
    const scroll = body.scrollTop;
    const typing = document.activeElement?.classList.contains('skl-search');
    bar.textContent = '';
    body.textContent = '';
    foot.textContent = '';
    bar.style.display = editing ? 'none' : '';
    if (editing) { drawEditor(); return; }
    const tabs = el('div', 'bbe-seg skl-tabs');
    for (const [key, label] of [['mine', `My skills (${mine.length})`], ['import', 'Import'], ['starters', 'Starters']] as Array<[Tab, string]>) {
      tabs.appendChild(button(`bbe-seg-b${tab === key ? ' on' : ''}`, label, () => {
        tab = key;
        say('');
        query = '';
        confirmDelete = '';
        if (key === 'import') scan();
        else run(draw());
      }));
    }
    const search = searchBox();
    const extra = tab === 'import'
      ? button('bbe-btn', 'From a folder…', pickFolder)
      : button('bbe-btn', '+ New skill', () => { editing = { old: null, text: NEW_SKILL, files: [] }; say(''); run(draw()); });
    bar.append(tabs, search, extra);
    if (typing) { search.focus(); search.setSelectionRange(query.length, query.length); }
    if (tab === 'mine') await drawMine(mine);
    else if (tab === 'import') drawImport();
    else await drawStarters();
    body.scrollTop = scroll;
  }

  const close = (): void => {
    ov.remove();
    document.removeEventListener('keydown', onKey, true);
    opts.onClose?.();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key !== 'Escape') return;
    e.stopPropagation();
    if (editing) { editing = null; run(draw()); } else close();
  };
  document.addEventListener('keydown', onKey, true);
  ov.addEventListener('mousedown', (e) => { if (e.target === ov && !editing) close(); });
  document.body.appendChild(ov);
  if (tab === 'import') scan();
  else run(draw());
}
