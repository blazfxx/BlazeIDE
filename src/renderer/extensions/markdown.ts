// A small, safe Markdown renderer for extension READMEs and changelogs.
// It builds DOM nodes (never innerHTML of the source), so nothing in a README
// can run. Pictures only come from `images` (data: URLs fetched by the Rust
// side); links open in the system browser. Inline HTML is reduced to its text,
// except <img> (shown from `images`) and <br>.

type Opts = { images: Record<string, string>; onLink: (url: string) => void };

const HTML_IMG = /<img\b[^>]*?\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))[^>]*>/gi;

function attr(tag: string, name: string): string {
  const m = new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, 'i').exec(tag);
  return m ? (m[1] ?? m[2] ?? m[3] ?? '') : '';
}

function picture(src: string, alt: string, o: Opts, width?: string): HTMLElement | null {
  const data = o.images[src];
  if (!data) return null;
  const img = document.createElement('img');
  img.src = data;
  img.alt = alt;
  img.loading = 'lazy';
  if (width && /^\d{1,4}$/.test(width)) img.style.width = `${Math.min(Number(width), 1200)}px`;
  return img;
}

function safeHref(url: string): string | null {
  const u = url.trim();
  return /^https?:\/\//i.test(u) || /^mailto:/i.test(u) ? u : null;
}

/** Inline markup: code, images, links, bold, italic, strike, autolinks, <img>, <br>. */
function inline(text: string, o: Opts, out: Node[] = []): Node[] {
  let i = 0;
  let buf = '';
  const flush = (): void => {
    if (buf) out.push(document.createTextNode(buf));
    buf = '';
  };
  while (i < text.length) {
    const c = text[i];
    const rest = text.slice(i);
    if (c === '\\' && i + 1 < text.length && /[\\`*_{}[\]()#+\-.!|<>~]/.test(text[i + 1])) {
      buf += text[i + 1];
      i += 2;
      continue;
    }
    if (c === '`') {
      const m = /^(`+)([\s\S]*?[^`])\1(?!`)/.exec(rest);
      if (m) {
        flush();
        const code = document.createElement('code');
        code.textContent = m[2].trim();
        out.push(code);
        i += m[0].length;
        continue;
      }
    }
    if (c === '<') {
      const img = /^<img\b[^>]*>/i.exec(rest);
      if (img) {
        flush();
        const src = attr(img[0], 'src');
        const p = picture(src, attr(img[0], 'alt'), o, attr(img[0], 'width'));
        if (p) out.push(p);
        i += img[0].length;
        continue;
      }
      if (/^<br\s*\/?>/i.test(rest)) {
        flush();
        out.push(document.createElement('br'));
        i += /^<br\s*\/?>/i.exec(rest)![0].length;
        continue;
      }
      const auto = /^<(https?:\/\/[^\s>]+)>/.exec(rest);
      if (auto) {
        flush();
        out.push(link(auto[1], [document.createTextNode(auto[1])], o));
        i += auto[0].length;
        continue;
      }
      const tag = /^<\/?[a-z][a-z0-9-]*\b[^>]*>/i.exec(rest);
      if (tag) {
        // Other inline HTML (<a>, <b>, <sub>…): keep the text, drop the tag.
        i += tag[0].length;
        continue;
      }
    }
    if (c === '!' && text[i + 1] === '[') {
      const m = /^!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+["'][^"']*["'])?\s*\)/.exec(rest);
      if (m) {
        flush();
        const p = picture(m[2], m[1], o);
        if (p) out.push(p);
        else if (m[1]) out.push(document.createTextNode(m[1]));
        i += m[0].length;
        continue;
      }
    }
    if (c === '[') {
      const m = /^\[((?:[^[\]]|\[[^\]]*\])*)\]\(\s*<?([^)\s>]+)>?(?:\s+["'][^"']*["'])?\s*\)/.exec(rest);
      if (m) {
        flush();
        out.push(link(m[2], inline(m[1], o), o));
        i += m[0].length;
        continue;
      }
    }
    if ((c === '*' || c === '_') && text[i + 1] === c) {
      const m = c === '*' ? /^\*\*(?=\S)([\s\S]*?\S)\*\*/.exec(rest) : /^__(?=\S)([\s\S]*?\S)__/.exec(rest);
      if (m) {
        flush();
        const b = document.createElement('strong');
        b.append(...inline(m[1], o));
        out.push(b);
        i += m[0].length;
        continue;
      }
    }
    if (c === '*' || (c === '_' && !/\w/.test(text[i - 1] ?? ''))) {
      const m = c === '*' ? /^\*(?=\S)([^*]*?\S)\*/.exec(rest) : /^_(?=\S)([^_]*?\S)_(?!\w)/.exec(rest);
      if (m) {
        flush();
        const em = document.createElement('em');
        em.append(...inline(m[1], o));
        out.push(em);
        i += m[0].length;
        continue;
      }
    }
    if (c === '~' && text[i + 1] === '~') {
      const m = /^~~(?=\S)([\s\S]*?\S)~~/.exec(rest);
      if (m) {
        flush();
        const s = document.createElement('del');
        s.append(...inline(m[1], o));
        out.push(s);
        i += m[0].length;
        continue;
      }
    }
    if (c === 'h' && /^https?:\/\//.test(rest) && !/[\w/]/.test(text[i - 1] ?? ' ')) {
      const m = /^https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"]/.exec(rest);
      if (m) {
        flush();
        out.push(link(m[0], [document.createTextNode(m[0])], o));
        i += m[0].length;
        continue;
      }
    }
    if (c === '&') {
      const m = /^&(amp|lt|gt|quot|#39|nbsp|copy|mdash|ndash|hellip);/.exec(rest);
      if (m) {
        buf += ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'", nbsp: ' ', copy: '©', mdash: '—', ndash: '–', hellip: '…' } as Record<string, string>)[m[1]];
        i += m[0].length;
        continue;
      }
    }
    buf += c;
    i++;
  }
  flush();
  return out;
}

function link(url: string, kids: Node[], o: Opts): HTMLElement {
  const href = safeHref(url);
  if (!href) {
    const span = document.createElement('span');
    span.append(...kids);
    return span;
  }
  const a = document.createElement('a');
  a.href = href;
  a.title = href;
  a.append(...kids);
  a.addEventListener('click', (e) => {
    e.preventDefault();
    o.onLink(href);
  });
  return a;
}

function para(lines: string[], o: Opts): HTMLElement {
  const p = document.createElement('p');
  lines.forEach((l, n) => {
    const hard = / {2,}$/.test(l) || /\\$/.test(l);
    p.append(...inline(l.replace(/ {2,}$|\\$/, ''), o));
    if (n < lines.length - 1) p.append(hard ? document.createElement('br') : document.createTextNode(' '));
  });
  return p;
}

const isBlank = (l: string): boolean => l.trim() === '';
const LIST = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;

function splitRow(l: string): string[] {
  let s = l.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  return s.split(/(?<!\\)\|/).map((c) => c.trim().replace(/\\\|/g, '|'));
}

function blocks(lines: string[], o: Opts, host: HTMLElement): void {
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (isBlank(line)) { i++; continue; }

    // Fenced code.
    const fence = /^\s*(`{3,}|~{3,})\s*([\w+#.-]*)/.exec(line);
    if (fence) {
      const close = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(close)) body.push(lines[i++]);
      i++;
      const pre = document.createElement('pre');
      const code = document.createElement('code');
      code.textContent = body.join('\n');
      if (fence[2]) pre.dataset.lang = fence[2];
      pre.appendChild(code);
      host.appendChild(pre);
      continue;
    }

    // HTML comment.
    if (line.trim().startsWith('<!--')) {
      while (i < lines.length && !lines[i].includes('-->')) i++;
      i++;
      continue;
    }

    // ATX heading.
    const h = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (h) {
      const el = document.createElement(`h${h[1].length}`);
      el.append(...inline(h[2], o));
      host.appendChild(el);
      i++;
      continue;
    }

    // HTML block heading (<h1 align=center>Title</h1>).
    const hh = /^\s*<h([1-6])\b[^>]*>(.*?)<\/h\1>\s*$/i.exec(line);
    if (hh) {
      const el = document.createElement(`h${hh[1]}`);
      el.append(...inline(hh[2], o));
      host.appendChild(el);
      i++;
      continue;
    }

    // Rule.
    if (/^\s{0,3}([-*_])(\s*\1){2,}\s*$/.test(line)) {
      host.appendChild(document.createElement('hr'));
      i++;
      continue;
    }

    // Quote (also GitHub's > [!NOTE] callouts, shown as a quote).
    if (/^\s{0,3}>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s{0,3}>/.test(lines[i])) body.push(lines[i++].replace(/^\s{0,3}>\s?/, ''));
      const q = document.createElement('blockquote');
      const first = /^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$/i.exec(body[0] ?? '');
      if (first) {
        body.shift();
        q.dataset.kind = first[1].toLowerCase();
        const t = document.createElement('div');
        t.className = 'md-callout';
        t.textContent = first[1][0] + first[1].slice(1).toLowerCase();
        q.appendChild(t);
      }
      blocks(body, o, q);
      host.appendChild(q);
      continue;
    }

    // Table.
    if (line.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(lines[i + 1])) {
      const head = splitRow(line);
      const aligns = splitRow(lines[i + 1]).map((c) => (c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : ''));
      i += 2;
      const wrap = document.createElement('div');
      wrap.className = 'md-table';
      const table = document.createElement('table');
      const thead = table.createTHead().insertRow();
      head.forEach((c, n) => {
        const th = document.createElement('th');
        if (aligns[n]) th.style.textAlign = aligns[n];
        th.append(...inline(c, o));
        thead.appendChild(th);
      });
      const tbody = table.createTBody();
      while (i < lines.length && lines[i].includes('|') && !isBlank(lines[i])) {
        const r = tbody.insertRow();
        splitRow(lines[i++]).forEach((c, n) => {
          const td = r.insertCell();
          if (aligns[n]) td.style.textAlign = aligns[n];
          td.append(...inline(c, o));
        });
      }
      wrap.appendChild(table);
      host.appendChild(wrap);
      continue;
    }

    // List (nested by indent).
    const lm = LIST.exec(line);
    if (lm) {
      const ordered = /\d/.test(lm[2]);
      const list = document.createElement(ordered ? 'ol' : 'ul');
      if (ordered && parseInt(lm[2], 10) !== 1) (list as HTMLOListElement).start = parseInt(lm[2], 10);
      const indent = lm[1].length;
      while (i < lines.length) {
        const m = LIST.exec(lines[i]);
        if (!m || m[1].length !== indent || /\d/.test(m[2]) !== ordered) break;
        const item: string[] = [m[3]];
        i++;
        while (i < lines.length) {
          const l = lines[i];
          const sub = LIST.exec(l);
          if (sub && sub[1].length <= indent) break;
          if (isBlank(l)) {
            const next = lines[i + 1];
            if (next !== undefined && (/^\s{2,}\S/.test(next) && (!LIST.exec(next) || LIST.exec(next)![1].length > indent))) { item.push(''); i++; continue; }
            break;
          }
          if (!sub && !/^\s/.test(l) && /^(#|>|```|~~~|\||<)/.test(l.trim())) break;
          item.push(l.replace(new RegExp(`^\\s{0,${indent + 4}}`), ''));
          i++;
        }
        const li = document.createElement('li');
        const task = /^\[([ xX])\]\s+/.exec(item[0]);
        if (task) {
          item[0] = item[0].slice(task[0].length);
          const box = document.createElement('input');
          box.type = 'checkbox';
          box.disabled = true;
          box.checked = task[1] !== ' ';
          li.appendChild(box);
          li.classList.add('task');
        }
        const inner = document.createElement('div');
        blocks(item, o, inner);
        // A tight list item is a single paragraph: unwrap it.
        if (inner.childElementCount === 1 && inner.firstElementChild?.tagName === 'P') li.append(...Array.from(inner.firstElementChild.childNodes));
        else li.append(...Array.from(inner.childNodes));
        list.appendChild(li);
      }
      host.appendChild(list);
      continue;
    }

    // Raw HTML block: keep pictures and text, centred when asked.
    if (/^\s*<(p|div|center|picture|a|img|table|details|summary|span|b|strong|sub|sup|br)\b/i.test(line) || /^\s*<\/\w+>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && !isBlank(lines[i])) body.push(lines[i++]);
      const html = body.join('\n');
      const div = document.createElement('div');
      div.className = 'md-html';
      if (/align\s*=\s*["']?center/i.test(html) || /<center>/i.test(html)) div.style.textAlign = 'center';
      // <picture><source …><img …></picture> and bare <img>: pictures; the rest as inline text.
      let last = 0;
      const parts: string[] = [];
      HTML_IMG.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = HTML_IMG.exec(html))) {
        parts.push(html.slice(last, m.index), m[0]);
        last = m.index + m[0].length;
      }
      parts.push(html.slice(last));
      for (const part of parts) {
        if (/^<img\b/i.test(part)) {
          div.append(...inline(part, o));
        } else {
          const text = part
            .replace(/<\/(p|div|h\d|li|tr|summary)>/gi, '\n')
            .replace(/<br\s*\/?>/gi, '\n')
            .replace(/<[^>]+>/g, '')
            .split('\n')
            .map((s) => s.trim())
            .filter(Boolean);
          text.forEach((t, n) => {
            div.append(...inline(t, o));
            if (n < text.length - 1) div.appendChild(document.createElement('br'));
          });
        }
      }
      if (div.childNodes.length) host.appendChild(div);
      continue;
    }

    // Indented code.
    if (/^( {4}|\t)/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && (/^( {4}|\t)/.test(lines[i]) || (isBlank(lines[i]) && /^( {4}|\t)/.test(lines[i + 1] ?? '')))) body.push(lines[i++].replace(/^( {4}|\t)/, ''));
      const pre = document.createElement('pre');
      const code = document.createElement('code');
      code.textContent = body.join('\n');
      pre.appendChild(code);
      host.appendChild(pre);
      continue;
    }

    // Paragraph (Setext headings too).
    const body: string[] = [];
    while (i < lines.length && !isBlank(lines[i])) {
      const l = lines[i];
      if (body.length && (/^\s{0,3}(#{1,6}\s|>|```|~~~)/.test(l) || LIST.exec(l))) break;
      if (body.length && /^\s{0,3}(=+|-+)\s*$/.test(l)) {
        const el = document.createElement(l.trim()[0] === '=' ? 'h1' : 'h2');
        el.append(...inline(body.join(' '), o));
        host.appendChild(el);
        body.length = 0;
        i++;
        break;
      }
      body.push(l);
      i++;
    }
    if (body.length) host.appendChild(para(body, o));
  }
}

export function renderMarkdown(src: string, opts: Opts): HTMLElement {
  const host = document.createElement('div');
  host.className = 'md';
  blocks(src.replace(/\r\n?/g, '\n').split('\n'), opts, host);
  return host;
}
