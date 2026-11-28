// Blaze on your phone: continue Blaze IDE / blaze CLI chats and answer the
// agent's questions. Plain script (no build step, no remote code): served by
// the computer itself, or by the BlazeAPI relay under /r/<id>/.
'use strict';

(function () {
  const BASE = location.pathname.replace(/[^/]*$/, '');
  const app = document.getElementById('app');
  const S = { name: '', projects: [], root: null, chats: [], chat: null, models: [], model: '', mode: 'agent', seq: 0, polling: false, live: null, pending: [], view: 'boot' };

  // ---------- helpers ----------
  function h(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = text;
    return e;
  }
  async function api(path, opts) {
    const o = opts || {};
    const init = { method: o.method || 'GET', credentials: 'same-origin', headers: {} };
    if (o.body !== undefined) {
      init.method = o.method || 'POST';
      init.headers['content-type'] = 'application/json';
      init.body = JSON.stringify(o.body);
    }
    if (init.method !== 'GET') init.headers['x-blaze'] = '1';
    const r = await fetch(BASE + path, init);
    let data = null;
    try { data = await r.json(); } catch (_) { data = null; }
    if (r.status === 401 && path !== 'api/login' && path !== 'api/pair') { showSignIn(); throw new Error('signed out'); }
    if (!r.ok) throw new Error((data && data.error) || ('Error ' + r.status));
    return data;
  }
  function ago(ms) {
    const s = Math.max(1, Math.round((Date.now() - ms) / 1000));
    if (s < 60) return 'just now';
    if (s < 3600) return Math.round(s / 60) + ' min ago';
    if (s < 86400) return Math.round(s / 3600) + ' h ago';
    return Math.round(s / 86400) + ' d ago';
  }
  // Minimal markdown: ``` code blocks, `inline code`, **bold**, - and 1. lists.
  function inline(parent, text) {
    String(text).split(/(`[^`\n]+`|\*\*[^*\n]+\*\*)/).forEach(function (seg) {
      if (seg.length > 1 && seg.startsWith('`') && seg.endsWith('`')) parent.appendChild(h('code', null, seg.slice(1, -1)));
      else if (seg.length > 4 && seg.startsWith('**') && seg.endsWith('**')) parent.appendChild(h('strong', null, seg.slice(2, -2)));
      else if (seg) parent.appendChild(document.createTextNode(seg));
    });
  }
  function md(text) {
    const box = h('div');
    const parts = String(text || '').split(/```/);
    parts.forEach(function (part, i) {
      if (i % 2 === 1) {
        const pre = h('pre');
        const code = h('code', null, part.replace(/^[a-zA-Z0-9_+-]*\n/, ''));
        pre.appendChild(code);
        box.appendChild(pre);
        return;
      }
      // Paragraphs split on blank lines; a run of "- x" / "1. x" lines is a list.
      part.replace(/^\n+|\n+$/g, '').split(/\n{2,}/).forEach(function (block) {
        if (!block.trim()) return;
        const lines = block.split('\n');
        let list = null;
        let para = null;
        lines.forEach(function (line) {
          const m = /^\s*([-*+]|\d{1,3}[.)])\s+(.*)$/.exec(line);
          if (m) {
            const tag = /\d/.test(m[1]) ? 'ol' : 'ul';
            if (!list || list.tagName.toLowerCase() !== tag) { list = h(tag); box.appendChild(list); }
            para = null;
            const li = h('li');
            inline(li, m[2]);
            list.appendChild(li);
          } else {
            list = null;
            if (!para) { para = h('p'); box.appendChild(para); } else para.appendChild(document.createTextNode(' '));
            inline(para, line);
          }
        });
      });
    });
    return box;
  }
  function screen(title, sub, back) {
    app.textContent = '';
    const s = h('div', 'screen');
    const top = h('div', 'top');
    if (back) {
      const b = h('button', 'ghost', '‹');
      b.setAttribute('aria-label', 'Back');
      b.addEventListener('click', back);
      top.appendChild(b);
    }
    const t = h('h1', null, title);
    if (sub) t.appendChild(h('span', 'sub', sub));
    top.appendChild(t);
    s.appendChild(top);
    const body = h('div', 'scroll');
    s.appendChild(body);
    app.appendChild(s);
    return { s: s, top: top, body: body };
  }

  // ---------- sign in ----------
  function showSignIn(msg) {
    S.view = 'signin';
    S.signedIn = false;
    app.textContent = '';
    const box = h('form', 'signin');
    const mark = h('img', 'mark');
    mark.src = 'icon.svg';
    mark.alt = '';
    box.append(mark, h('h2', null, 'Continue on mobile'), h('p', null, 'Enter the password you chose on your computer (Blaze IDE: Settings › Continue on mobile, or `blaze mobile`).'));
    const pw = h('input');
    pw.type = 'password';
    pw.autocomplete = 'current-password';
    pw.placeholder = 'Password';
    const err = h('div', 'err', msg || '');
    const go = h('button', 'primary', 'Sign in');
    go.type = 'submit';
    box.append(pw, err, go);
    box.addEventListener('submit', async function (e) {
      e.preventDefault();
      go.disabled = true;
      err.textContent = '';
      try {
        await api('api/login', { body: { password: pw.value } });
        pw.value = '';
        start();
      } catch (x) {
        err.textContent = x.message;
      } finally { go.disabled = false; }
    });
    app.appendChild(box);
    setTimeout(function () { pw.focus(); }, 50);
  }

  // ---------- projects ----------
  async function showProjects() {
    S.view = 'projects';
    S.chat = null;
    const v = screen('Blaze', S.name);
    const out = h('button', 'ghost', 'Sign out');
    out.addEventListener('click', async function () { try { await api('api/logout', { body: {} }); } catch (_) {} showSignIn('Signed out.'); });
    v.top.appendChild(out);
    v.body.appendChild(h('div', 'section', 'Projects'));
    const list = h('div', 'list');
    v.body.appendChild(list);
    try {
      const d = await api('api/projects');
      S.projects = d.projects || [];
    } catch (x) { list.appendChild(h('div', 'error', x.message)); return; }
    if (!S.projects.length) list.appendChild(h('div', 'empty', 'No projects yet. Open a folder in Blaze IDE (or run `blaze` in one) on your computer first.'));
    S.projects.forEach(function (p) {
      const b = h('button', 'item');
      b.append(h('span', 't', p.name), h('span', 's', p.root));
      b.addEventListener('click', function () { S.root = p.root; showChats(); });
      list.appendChild(b);
    });
    try {
      const b = await api('api/bots');
      if ((b.bots || []).length) {
        v.body.appendChild(h('div', 'section', 'BlBots'));
        const bl = h('div', 'list');
        b.bots.forEach(function (bot) {
          const it = h('button', 'item bot');
          const top = h('span', 't');
          const av = h('span', 'avatar', bot.avatar || bot.name.slice(0, 1));
          if (/^#[0-9a-fA-F]{6}$/.test(bot.color || '')) av.style.background = bot.color;
          top.append(av, document.createTextNode(bot.name));
          it.append(top, h('span', 's', (bot.title ? bot.title + ' · ' : '') + bot.engine));
          it.addEventListener('click', function () { showBot(bot.id); });
          bl.appendChild(it);
        });
        v.body.appendChild(bl);
      }
      if ((b.rooms || []).length) {
        v.body.appendChild(h('div', 'section', 'Rooms'));
        const rl = h('div', 'list');
        b.rooms.forEach(function (r) {
          const it = h('button', 'item');
          it.append(h('span', 't', '# ' + r.name), h('span', 's', r.members.length + ' bots'));
          it.addEventListener('click', function () { S.target = { kind: 'room', room: r.id }; openChat(r.id); });
          rl.appendChild(it);
        });
        v.body.appendChild(rl);
      }
    } catch (_) { /* bots are optional */ }
    v.body.appendChild(h('div', 'foot', 'Chats run on your computer and use its BlazeAPI account.'));
  }

  // ---------- one bot ----------
  async function showBot(botId) {
    S.view = 'bot';
    S.chat = null;
    S.target = { kind: 'bot', bot: botId };
    let d;
    try { d = await api('api/bot/threads?bot=' + encodeURIComponent(botId)); } catch (x) { alert(x.message); return showProjects(); }
    const v = screen(d.bot.name, d.bot.title || d.bot.engine, showProjects);
    const nb = h('button', 'primary', 'New chat');
    nb.addEventListener('click', function () { openChat(null); });
    v.top.appendChild(nb);
    const list = h('div', 'list');
    v.body.appendChild(list);
    if (!d.threads.length) list.appendChild(h('div', 'empty', 'No chats with ' + d.bot.name + ' yet.'));
    d.threads.forEach(function (c) {
      const b = h('button', 'item');
      const t = h('span', 't', c.title || 'Untitled');
      if (c.working) t.appendChild(h('span', 'badge', 'working'));
      b.append(t, h('span', 's', c.messages + ' messages · ' + ago(c.updatedAt)));
      b.addEventListener('click', function () { openChat(c.id); });
      list.appendChild(b);
    });
  }

  // ---------- chats ----------
  // What the list or chat on screen was drawn from, to redraw only on change.
  function listKey(chats) { return (chats || []).map(function (c) { return [c.id, c.updatedAt, c.messages, c.working].join(); }).join(';'); }
  function chatKey(d) { return [d.messages.length, d.live, d.runId || '', JSON.stringify(d.messages[d.messages.length - 1] || '').length].join(); }

  // Chats worked on in Blaze IDE do not send events: look again every few seconds.
  setInterval(async function () {
    if (document.hidden || !S.signedIn || S.refreshing) return;
    S.refreshing = true;
    try {
      if (S.view === 'chats') {
        const root = S.root;
        const d = await api('api/chats?root=' + encodeURIComponent(root));
        if (S.view === 'chats' && S.root === root && listKey(d.chats) !== S.listKey) showChats(d.chats);
      } else if (S.view === 'chat' && S.chat && S.chat.id && !S.chat.runId && (!S.target || S.target.kind === 'project')) {
        const chat = S.chat;
        const d = await api('api/chat?root=' + encodeURIComponent(S.root) + '&id=' + encodeURIComponent(chat.id));
        if (S.chat === chat && !chat.runId && chatKey(d) !== chat.key) {
          const stick = nearBottom();
          const top = scroller ? scroller.scrollTop : 0;
          const draft = composer ? composer.input.value : '';
          Object.assign(chat, { title: d.title, messages: d.messages, runId: d.runId || null, live: !!d.live, key: chatKey(d) });
          S.pending = d.pending || [];
          renderChat();
          composer.input.value = draft;
          if (!stick) scroller.scrollTop = top;
        }
      }
    } catch (_) { /* try again next time */ }
    S.refreshing = false;
  }, 3000);

  async function showChats(known) {
    S.view = 'chats';
    S.chat = null;
    S.target = { kind: 'project', root: S.root };
    const proj = S.projects.find(function (p) { return p.root === S.root; });
    const v = screen(proj ? proj.name : 'Chats', 'Chats', showProjects);
    const nb = h('button', 'primary', 'New chat');
    nb.addEventListener('click', function () { openChat(null); });
    v.top.appendChild(nb);
    const list = h('div', 'list');
    v.body.appendChild(list);
    try {
      S.chats = Array.isArray(known) ? known : (await api('api/chats?root=' + encodeURIComponent(S.root))).chats || [];
      S.listKey = listKey(S.chats);
    } catch (x) { list.appendChild(h('div', 'error', x.message)); return; }
    if (!S.chats.length) list.appendChild(h('div', 'empty', 'No chats in this project yet. Start one.'));
    S.chats.forEach(function (c) {
      const b = h('button', 'item');
      const t = h('span', 't', c.title || 'Untitled');
      if (c.working) t.appendChild(h('span', 'badge', 'working'));
      b.append(t, h('span', 's', c.messages + ' messages · ' + ago(c.updatedAt)));
      b.addEventListener('click', function () { openChat(c.id); });
      list.appendChild(b);
    });
  }

  // ---------- one chat ----------
  async function openChat(id) {
    S.view = 'chat';
    S.chat = { id: id, title: 'New chat', messages: [], runId: null };
    S.pending = [];
    const T = S.target || { kind: 'project' };
    if (id) {
      try {
        const url = T.kind === 'bot' ? 'api/bot/thread?bot=' + encodeURIComponent(T.bot) + '&id=' + encodeURIComponent(id)
          : T.kind === 'room' ? 'api/room?id=' + encodeURIComponent(T.room)
          : 'api/chat?root=' + encodeURIComponent(S.root) + '&id=' + encodeURIComponent(id);
        const d = await api(url);
        S.chat = { id: d.id, title: d.title, messages: d.messages, runId: d.runId || null, live: !!d.live, key: chatKey(d) };
        S.members = {};
        (d.members || []).forEach(function (b) { S.members[b.id] = b.name; });
        S.model = d.model || S.model;
        S.mode = d.mode || 'agent';
        S.pending = d.pending || [];
        if (d.seq > S.seq) S.seq = d.seq;
      } catch (x) { alert(x.message); return back(); }
    }
    renderChat();
  }

  let msgsBox = null, scroller = null, composer = null, workingRow = null;

  function back() {
    const T = S.target || { kind: 'project' };
    if (T.kind === 'bot') return showBot(T.bot);
    if (T.kind === 'room') return showProjects();
    return showChats();
  }

  function renderChat() {
    const v = screen(S.chat.title || 'New chat', S.chat.runId ? 'working…' : S.chat.live ? 'working in Blaze IDE…' : '', back);
    scroller = v.body;
    msgsBox = h('div', 'msgs');
    v.body.appendChild(msgsBox);
    S.chat.messages.forEach(addMessage);
    S.pending.forEach(addCard);
    workingRow = h('div', 'working hidden');
    workingRow.append(h('span', 'spin'), h('span', null, 'Working on your computer…'));
    v.body.appendChild(workingRow);
    composer = buildComposer();
    v.s.appendChild(composer.el);
    setWorking(!!S.chat.runId);
    if (S.chat.live && !S.chat.runId) {
      // The IDE has the chat: show progress, send when it is done.
      workingRow.classList.remove('hidden');
      composer.send.disabled = true;
      composer.input.placeholder = 'Blaze IDE is working on this…';
    }
    toBottom();
  }

  function addMessage(m) {
    if (m.role === 'user') { msgsBox.appendChild(h('div', 'msg user', m.text)); return; }
    if (m.text && m.text.trim()) {
      const d = h('div', 'msg assistant');
      if (m.author) d.appendChild(h('div', 'author', m.author));
      d.appendChild(md(m.text));
      msgsBox.appendChild(d);
    }
    (m.tools || []).forEach(function (t) { msgsBox.appendChild(toolRow(t, 'ok')); });
  }
  function toolRow(text, state) {
    const r = h('div', 'tool' + (state === 'run' ? ' run' : state === 'fail' ? ' fail' : ''));
    r.append(h('span', 'dot', state === 'fail' ? '✗' : '●'), h('span', null, text));
    return r;
  }
  function toBottom() { if (scroller) scroller.scrollTop = scroller.scrollHeight; }
  function nearBottom() { return !scroller || scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 120; }

  function setWorking(on) {
    if (workingRow) workingRow.classList.toggle('hidden', !on);
    if (composer) {
      composer.send.textContent = on ? '■' : '↑';
      composer.send.setAttribute('aria-label', on ? 'Stop' : 'Send');
    }
  }

  function addCard(p) {
    if (!msgsBox || document.getElementById('card-' + p.id)) return;
    const c = h('div', 'card');
    c.id = 'card-' + p.id;
    if (p.kind === 'approve') {
      c.append(h('div', 'q', 'Allow this?'), h('div', null, p.summary));
      const row = h('div', 'row');
      const yes = h('button', 'primary', 'Allow');
      const no = h('button', 'danger', 'Refuse');
      yes.addEventListener('click', function () { answer(p.id, { allow: true }, c); });
      no.addEventListener('click', function () { answer(p.id, { allow: false }, c); });
      row.append(no, yes);
      c.appendChild(row);
    } else {
      const answers = [];
      const qs = p.questions || [];
      const ask = function (i) {
        c.textContent = '';
        const q = qs[i];
        c.appendChild(h('div', 'q', q.question));
        const opts = h('div', 'opts');
        (q.options || []).forEach(function (o) {
          const b = h('button', null, o);
          b.addEventListener('click', function () { next(o); });
          opts.appendChild(b);
        });
        const inp = h('input');
        inp.placeholder = 'Or type an answer';
        const send = h('button', 'primary', 'Answer');
        send.addEventListener('click', function () { if (inp.value.trim()) next(inp.value.trim()); });
        c.append(opts, inp, send);
      };
      const next = function (a) {
        answers.push(a);
        if (answers.length < qs.length) ask(answers.length);
        else answer(p.id, { answers: answers }, c);
      };
      ask(0);
    }
    msgsBox.appendChild(c);
    toBottom();
  }
  async function answer(id, body, card) {
    card.querySelectorAll('button').forEach(function (b) { b.disabled = true; });
    try { await api('api/answer', { body: Object.assign({ id: id }, body) }); } catch (x) { /* already answered elsewhere */ }
    card.remove();
    S.pending = S.pending.filter(function (p) { return p.id !== id; });
  }

  function buildComposer() {
    const el = h('div', 'composer');
    const opts = h('div', 'opts');
    const model = h('select');
    const mode = h('select');
    [['agent', 'Agent'], ['plan', 'Plan'], ['ask', 'Ask']].forEach(function (m) {
      const o = h('option', null, m[1]); o.value = m[0]; if (m[0] === S.mode) o.selected = true; mode.appendChild(o);
    });
    mode.addEventListener('change', function () { S.mode = mode.value; });
    const fillModels = function () {
      model.textContent = '';
      let list = S.models.length ? S.models : [{ id: S.model || 'default', name: S.model || 'Default model' }];
      // The computer may name its model without the catalogue prefix
      // ("claude-sonnet-5-5" vs "payg/claude-sonnet-5-5"): match either way, and
      // never show a different model than the one that will be used.
      const same = function (id) { return id === S.model || id.split('/').pop() === String(S.model).split('/').pop(); };
      let hit = list.find(function (m) { return m.id === S.model; }) || list.find(function (m) { return same(m.id); });
      if (S.model && !hit) { hit = { id: S.model, name: S.model }; list = [hit].concat(list); }
      list.forEach(function (m) { const o = h('option', null, m.name || m.id); o.value = m.id; if (hit && m.id === hit.id) o.selected = true; model.appendChild(o); });
    };
    fillModels();
    if (!S.models.length) api('api/models').then(function (d) { S.models = d.models || []; if (!S.model) S.model = d.default; fillModels(); }).catch(function () {});
    model.addEventListener('change', function () { S.model = model.value; });
    if (!S.target || S.target.kind === 'project') opts.append(model, mode);
    const bar = h('div', 'bar');
    const ta = h('textarea');
    ta.rows = 1;
    ta.placeholder = 'Message';
    ta.addEventListener('input', function () { ta.style.height = 'auto'; ta.style.height = Math.min(ta.scrollHeight, window.innerHeight * 0.4) + 'px'; });
    const send = h('button', 'send primary', '↑');
    send.addEventListener('click', async function () {
      if (S.chat.runId) {
        try { await api('api/stop', { body: { runId: S.chat.runId } }); } catch (_) {}
        return;
      }
      const text = ta.value.trim();
      if (!text) return;
      send.disabled = true;
      try {
        const T = S.target || { kind: 'project' };
        const d = T.kind === 'bot' ? await api('api/bot/send', { body: { bot: T.bot, thread: S.chat.id, text: text } })
          : T.kind === 'room' ? await api('api/room/send', { body: { room: T.room, text: text } })
          : await api('api/send', { body: { root: S.root, chatId: S.chat.id, text: text, model: S.model, mode: S.mode } });
        ta.value = '';
        ta.style.height = '';
        S.chat.id = d.chatId;
        S.chat.runId = d.runId;
        addMessage({ role: 'user', text: text });
        S.live = null;
        setWorking(true);
        toBottom();
      } catch (x) {
        msgsBox.appendChild(h('div', 'error', x.message));
      } finally { send.disabled = false; }
    });
    bar.append(ta, send);
    el.append(opts, bar);
    return { el: el, send: send, input: ta };
  }

  // ---------- live events (long poll) ----------
  function onEvent(e) {
    if (S.view === 'chats' && (e.type === 'run_start' || e.type === 'run_end')) { showChats(); return; }

    if (S.view === 'bot' && e.type === 'run_end' && S.target) { showBot(S.target.bot); return; }
    if (S.view !== 'chat' || !S.chat) return;
    const mine = e.chatId === S.chat.id || (e.item && e.item.chatId === S.chat.id);
    if (e.type === 'run_start' && e.chatId === S.chat.id) { S.chat.runId = e.runId; setWorking(true); return; }
    if (!mine && e.type !== 'answered') return;
    const stick = nearBottom();
    if (e.type === 'agent') {
      const ev = e.ev || {};
      if (ev.type === 'text') {
        if (S.live && e.bot && S.live.bot !== e.bot) S.live = null;
        if (!S.live) { S.live = { el: h('div', 'msg assistant'), text: '', bot: e.bot }; msgsBox.appendChild(S.live.el); }
        S.live.text += ev.text;
        S.live.el.textContent = '';
        if (S.target && S.target.kind === 'room' && e.bot && S.members) S.live.el.appendChild(h('div', 'author', S.members[e.bot] || 'bot'));
        S.live.el.appendChild(md(S.live.text));
      } else if (ev.type === 'tool_start') {
        S.live = null;
        const r = toolRow(ev.summary, 'run');
        r.dataset.id = ev.id;
        msgsBox.appendChild(r);
      } else if (ev.type === 'tool_end') {
        const r = msgsBox.querySelector('.tool[data-id="' + CSS.escape(ev.id) + '"]');
        if (r) {
          r.className = 'tool' + (ev.is_error ? ' fail' : '');
          r.querySelector('.dot').textContent = ev.is_error ? '✗' : '●';
          if (ev.is_error) r.appendChild(h('span', 'why', (ev.result || '').split('\n')[0].slice(0, 160)));
        }
      } else if (ev.type === 'file_changed') {
        const f = h('div', 'file');
        f.append(document.createTextNode((ev.created ? 'new ' : 'edit ') + ev.rel + '  '), h('span', 'a', '+' + ev.added), document.createTextNode(' '), h('span', 'r', '-' + ev.removed));
        msgsBox.appendChild(f);
      } else if (ev.type === 'notice') {
        msgsBox.appendChild(h('div', 'notice', ev.text));
      } else if (ev.type === 'error') {
        msgsBox.appendChild(h('div', 'error', ev.message));
      }
    } else if (e.type === 'pending') {
      S.pending.push(e.item);
      addCard(e.item);
    } else if (e.type === 'answered') {
      const c = document.getElementById('card-' + e.id);
      if (c) c.remove();
    } else if (e.type === 'run_end') {
      S.chat.runId = null;
      S.live = null;
      setWorking(false);
      if (e.error) msgsBox.appendChild(h('div', 'error', e.error));
      if (e.stopped) msgsBox.appendChild(h('div', 'notice', 'Stopped.'));
      if (e.title) { const t = document.querySelector('.top h1'); if (t && t.firstChild) t.firstChild.textContent = e.title; }
    }
    if (stick) toBottom();
  }

  async function poll() {
    if (S.polling) return;
    S.polling = true;
    let fails = 0;
    S.signedIn = true;
    while (S.signedIn) {
      try {
        const d = await api('api/events?after=' + S.seq);
        fails = 0;
        if (d.seq < S.seq) S.seq = 0; // the computer restarted
        (d.events || []).forEach(function (e) { if (e.seq > S.seq) S.seq = e.seq; onEvent(e); });
        if (!d.events || !d.events.length) S.seq = Math.max(S.seq, d.seq);
      } catch (x) {
        if (x.message === 'signed out') break;
        fails++;
        await new Promise(function (r) { setTimeout(r, Math.min(15000, 1000 * fails)); });
      }
    }
    S.polling = false;
  }

  // ---------- start ----------
  async function start() {
    try {
      const me = await api('api/me');
      S.name = me.name || '';
      S.model = S.model || me.defaultModel || '';
    } catch (x) { if (x.message !== 'signed out') showSignIn(x.message); return; }
    try { const e = await api('api/events?after=999999999999'); S.seq = e.seq || 0; } catch (_) {}
    showProjects();
    poll();
  }

  async function boot() {
    const m = /[#&]pair=([A-Za-z0-9_-]+)/.exec(location.hash);
    if (m) {
      history.replaceState(null, '', location.pathname);
      try { await api('api/pair', { body: { code: m[1] } }); }
      catch (x) { showSignIn(x.message); return; }
    }
    start();
  }
  document.addEventListener('visibilitychange', function () { if (!document.hidden && S.view === 'chat' && S.chat && S.chat.id && !S.chat.runId) openChat(S.chat.id); });
  boot();
})();
