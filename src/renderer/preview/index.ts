// Live web preview: renders a local HTML file (via blazemedia:) or any
// localhost dev-server URL in a sandboxed iframe, with a URL bar and a
// "Select element" mode. The IDE and the iframe are different origins, so
// selection runs via a script injected into the page (server-side, by the
// blazemedia: handler in Rust) that talks to the IDE over postMessage.

import { bus, el, state } from '../state';
import type { EditorArea } from '../editor/index';
import type { ElementComment } from '../../shared/types';
import { icon, iconAgent } from '../icons/index';

const TAB_ID = 'preview';

const LOOPBACK = /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])(?::(\d+))?(\/[^#]*)?/i;

/** A localhost dev server is shown through the IDE's blazeweb: proxy, so the
 *  preview helpers (picker, the agent's browser) work on it too. */
function proxiedUrl(v: string): string | null {
  // UI tests run in a plain browser, which cannot load the blazeweb: scheme.
  if ((window as unknown as { __BLAZE_NO_PROXY__?: boolean }).__BLAZE_NO_PROXY__) return null;
  const m = LOOPBACK.exec(v);
  if (!m) return null;
  const host = m[1].toLowerCase();
  const port = m[2] || (v.toLowerCase().startsWith('https') ? '443' : '80');
  const base = /Windows/i.test(navigator.userAgent) ? 'http://blazeweb.localhost' : 'blazeweb://localhost';
  return `${base}/${host}:${port}${m[3] || '/'}`;
}

const BARE_LOOPBACK = /^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/.*)?$/i;

/** What the address bar shows: "localhost:3000/about" or a project path, never
 *  the IDE's internal blazeweb: / blazemedia: addresses. */
export function displayUrl(url: string): string {
  const web = /^(?:blazeweb:\/\/localhost|http:\/\/blazeweb\.localhost)\/(.*)$/i.exec(url);
  if (web) return web[1].replace(/^(localhost|127\.0\.0\.1|\[::1\]):(80|443)(?=\/|$)/i, '$1');
  const media = /^(?:blazemedia:\/\/localhost|http:\/\/blazemedia\.localhost)(\/.*)$/i.exec(url);
  if (media) {
    let path = media[1];
    try { path = decodeURIComponent(path.split(/[?#]/)[0]); } catch { /* keep as it is */ }
    if (/^\/[A-Za-z]:\//.test(path)) path = path.slice(1);
    const root = (state.projectRoot ?? '').replace(/\\/g, '/');
    return root && path.startsWith(`${root}/`) ? path.slice(root.length + 1) : path;
  }
  return url.replace(/^http:\/\/(?=localhost|127\.0\.0\.1|\[::1\])/i, '');
}

function toPreviewUrl(input: string): string {
  let v = input.trim();
  if (BARE_LOOPBACK.test(v)) v = `http://${v}`;
  const proxied = proxiedUrl(v);
  if (proxied) return proxied;
  if (/^blazeweb:/i.test(v) || /^http:\/\/blazeweb\.localhost\//i.test(v)) return v;
  if (/^https?:\/\//i.test(v)) return v;
  if (/^blazemedia:/i.test(v) || /^http:\/\/blazemedia\.localhost\//i.test(v)) return v;
  const isAbs = v.startsWith('/') || /^[A-Za-z]:[\\/]/.test(v);
  const abs = isAbs ? v : `${state.projectRoot ?? ''}/${v}`;
  return window.void.mediaUrl(abs);
}

function defaultUrl(editor: EditorArea): string {
  const active = editor.getActive();
  if (active && /\.html?$/i.test(active.path)) return toPreviewUrl(active.path);
  if (state.projectRoot) return toPreviewUrl(`${state.projectRoot}/index.html`);
  return '';
}

// --- the AI agent's browser -------------------------------------------------
// The agent drives the same Preview tab the user sees. Project HTML pages get
// a helper injected by the Rust blazemedia: handler (preview_agent.js) that
// answers 'void:agent' requests: console, click, scroll, screenshot.
interface PreviewHandle { frame: HTMLIFrameElement; urlInput: HTMLInputElement; load: () => void; setWidth: (w: number | null) => void }

const DEVICES: Array<{ key: string; label: string; w: number | null; h: number | null }> = [
  { key: 'fit', label: 'Fit', w: null, h: null },
  { key: 'phone', label: 'Phone', w: 390, h: 844 },
  { key: 'tablet', label: 'Tablet', w: 820, h: 1180 },
  { key: 'desktop', label: 'Desktop', w: 1440, h: null },
];
const DEVICE_KEY = 'blaze-preview-device';
function savedDevice(): string {
  try { return localStorage.getItem(DEVICE_KEY) ?? 'fit'; } catch { return 'fit'; }
}

/** Where the frame goes in the stage: a size larger than the room is scaled
 *  down, so a page is never cut off. */
export function frameLayout(stageW: number, stageH: number, w: number | null, h: number | null): { width: number; height: number; left: number; top: number; scale: number } {
  if (!w) return { width: stageW, height: stageH, left: 0, top: 0, scale: 1 };
  const pad = w < 1000 ? 16 : 0;
  const roomW = Math.max(1, stageW - pad * 2);
  const roomH = Math.max(1, stageH - pad * 2);
  const scale = Math.min(1, roomW / w, h ? Math.max(0.5, roomH / h) : 1);
  const height = h ? Math.min(h, Math.floor(roomH / scale)) : Math.floor(roomH / scale);
  return { width: w, height, scale, left: Math.max(0, Math.round((stageW - w * scale) / 2)), top: pad };
}

const svg = (d: string): string => `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
const ICON_BACK = svg('<path d="M19 12H5M11 18l-6-6 6-6"/>');
const ICON_FORWARD = svg('<path d="M5 12h14M13 6l6 6-6 6"/>');
const ICON_RELOAD = svg('<path d="M20 11a8 8 0 1 0-2.3 6.4M20 4v7h-7"/>');
const ICON_EXTERNAL = svg('<path d="M14 4h6v6M20 4l-9 9M19 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1h5"/>');
let current: PreviewHandle | null = null;
let editorRef: EditorArea | null = null;

function waitForLoad(frame: HTMLIFrameElement, ms: number): Promise<boolean> {
  return new Promise((resolve) => {
    const t = window.setTimeout(() => { frame.removeEventListener('load', on); resolve(false); }, ms);
    const on = (): void => { window.clearTimeout(t); frame.removeEventListener('load', on); resolve(true); };
    frame.addEventListener('load', on);
  });
}

/** Ask the injected helper in the preview page; null when it does not answer. */
function askPage(op: string, extra: Record<string, unknown>, ms: number): Promise<{ ok: boolean; data: unknown } | null> {
  const frame = current?.frame;
  if (!frame?.contentWindow) return Promise.resolve(null);
  const id = crypto.randomUUID();
  return new Promise((resolve) => {
    const done = (v: { ok: boolean; data: unknown } | null): void => { window.removeEventListener('message', on); window.clearTimeout(t); resolve(v); };
    const on = (e: MessageEvent): void => {
      if (e.source !== frame.contentWindow) return;
      const d = e.data as { type?: string; id?: string; ok?: boolean; data?: unknown } | null;
      if (d && d.type === 'void:agent-result' && d.id === id) done({ ok: !!d.ok, data: d.data });
    };
    const t = window.setTimeout(() => done(null), ms);
    window.addEventListener('message', on);
    frame.contentWindow?.postMessage({ type: 'void:agent', id, op, ...extra }, '*');
  });
}

export interface BrowserResult { text: string; image?: string; isError?: boolean }

const NO_HELPER = 'The page did not answer the browser helper (it may still be loading, or it is not an HTML page). Try open again, or check that the file / dev server exists.';

/** The Preview frame, visible and connected. Re-opens the tab (reloading the
 *  page) if it was closed or another tab is in front, since a hidden frame
 *  cannot load or be screenshotted. */
async function ensureVisible(): Promise<PreviewHandle | null> {
  const h = current;
  if (h && h.frame.isConnected && h.frame.offsetParent !== null) return h;
  if (!editorRef || !h) return null;
  const url = h.urlInput.value;
  openPreview(editorRef, url);
  const fresh = current as PreviewHandle | null;
  if (!fresh) return null;
  await waitForLoad(fresh.frame, 15000);
  await new Promise((r) => setTimeout(r, 700));
  return fresh;
}

/** One browser-tool action from the AI agent. */
export async function agentBrowser(args: { action?: string; target?: string; y?: number; full_page?: boolean; width?: number }): Promise<BrowserResult> {
  const action = String(args.action || '');
  if (!editorRef) return { text: 'Error: the preview is not available.', isError: true };
  if (action === 'open') {
    const target = String(args.target || '').trim();
    if (!target) return { text: 'Error: open needs `target`: a project path such as index.html, or an http://localhost URL.', isError: true };
    if (/^https?:\/\//i.test(target) && !LOOPBACK.test(target)) {
      return { text: 'Error: the browser only opens project files and localhost URLs.', isError: true };
    }
    // Always a fresh, visible Preview tab: a frame from an earlier (closed or
    // hidden) tab never finishes loading.
    openPreview(editorRef, target);
    const h = current as PreviewHandle | null;
    if (!h) return { text: 'Error: could not open the preview.', isError: true };
    const loaded = waitForLoad(h.frame, 20000);
    h.setWidth(typeof args.width === 'number' && args.width >= 240 ? Math.min(args.width, 2560) : null);
    const ok = await loaded;
    await new Promise((r) => setTimeout(r, 700)); // let fonts/images/layout settle
    let info = await askPage('console', {}, 2500);
    if (!info) { await new Promise((r) => setTimeout(r, 1500)); info = await askPage('console', {}, 2500); }
    if (!info) return { text: `${ok ? 'Opened' : 'Tried to open (it did not finish loading in 20 s)'} ${target}. ${NO_HELPER}` };
    const d = info.data as { title?: string; errors?: number };
    return { text: `Opened ${target}${d.title ? ` ("${d.title}")` : ''}. ${d.errors ? `${d.errors} console error(s): use action console to read them.` : 'No console errors.'} Take a screenshot to see it.` };
  }
  if (!current) return { text: 'Error: no page is open. Use action open first.', isError: true };
  const vis = await ensureVisible();
  if (!vis) return { text: 'Error: the Preview tab is gone. Use action open again.', isError: true };
  if (action === 'screenshot') {
    if (typeof args.width === 'number' && args.width >= 240) {
      current.setWidth(Math.min(args.width, 2560));
      await new Promise((r) => setTimeout(r, 500));
    }
    const r = await askPage('screenshot', { full: !!args.full_page }, 30000);
    if (!r) return { text: `Error: no screenshot. ${NO_HELPER}`, isError: true };
    if (!r.ok) return { text: `Error: ${String(r.data)}`, isError: true };
    const d = r.data as { url: string; width: number; height: number; scrollY: number; pageHeight: number; errors: number };
    return {
      text: `Screenshot of ${current.urlInput.value}: ${d.width}x${d.height} px${args.full_page ? ' (whole page)' : ` viewport at scroll y=${d.scrollY}`}, page is ${d.pageHeight} px tall.${d.errors ? ` ${d.errors} console error(s).` : ''} The image is attached below.`,
      image: d.url,
    };
  }
  if (action === 'console') {
    const r = await askPage('console', {}, 3000);
    if (!r) return { text: `Error: ${NO_HELPER}`, isError: true };
    const d = r.data as { logs: string[]; url: string };
    return { text: d.logs.length ? `Console of ${d.url} (newest last):\n${d.logs.join('\n')}` : 'The console is empty (no logs or errors).' };
  }
  if (action === 'click' || action === 'scroll') {
    const r = await askPage(action, action === 'click' ? { selector: String(args.target || '') } : { y: Number(args.y) || 0 }, 5000);
    if (!r) return { text: `Error: ${NO_HELPER}`, isError: true };
    return { text: r.ok ? String(r.data) : `Error: ${String(r.data)}`, isError: !r.ok };
  }
  return { text: 'Error: unknown action. Use open, screenshot, console, click or scroll.', isError: true };
}

export function initPreview(editor: EditorArea): void {
  editorRef = editor;
  bus.on('open-preview', ({ url }) => openPreview(editor, url));
}

function openPreview(editor: EditorArea, initialUrl?: string): void {
  const start = initialUrl ? toPreviewUrl(initialUrl) : defaultUrl(editor);

  editor.openCustomTab(TAB_ID, 'Preview', (host) => {
    host.style.cssText = 'display:flex;flex-direction:column;height:100%;position:relative;min-width:0;';

    const button = (cls: string, html: string, title: string): HTMLButtonElement => {
      const b = el('button', cls, html) as HTMLButtonElement;
      b.type = 'button';
      b.title = title;
      b.setAttribute('aria-label', title);
      return b;
    };
    const bar = el('div', 'preview-bar');
    const backBtn = button('preview-btn pv-back', ICON_BACK, 'Back');
    const fwdBtn = button('preview-btn pv-forward', ICON_FORWARD, 'Forward');
    const reloadBtn = button('preview-btn pv-reload', ICON_RELOAD, 'Reload');
    const urlInput = el('input', 'preview-url') as HTMLInputElement;
    urlInput.spellcheck = false;
    urlInput.placeholder = 'localhost:3000 or a file such as index.html';
    urlInput.setAttribute('aria-label', 'Address');
    urlInput.value = displayUrl(start);
    const externalBtn = button('preview-btn pv-external', ICON_EXTERNAL, 'Open in your web browser');
    const devices = el('div', 'pv-devices');
    const size = el('span', 'pv-size');
    const selectBtn = button('preview-btn preview-select', `${icon(iconAgent, 'icon-12')}<span>Comment</span>`, 'Click something in the page to tell the AI what to change');
    const hint = el('div', 'preview-hint');
    bar.append(backBtn, fwdBtn, reloadBtn, urlInput, externalBtn, devices, size, selectBtn);

    const stage = el('div', 'pv-stage');
    const frame = el('iframe', 'preview-frame') as HTMLIFrameElement;
    frame.title = 'Page preview';
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-forms allow-modals');
    const empty = el('div', 'pv-empty');
    empty.append(el('div', 'pv-empty-title', 'Nothing to preview yet'), el('div', '', 'Start your dev server and type its address above, such as localhost:3000, or type the path of an HTML file in your project.'));
    stage.append(frame, empty);
    host.append(bar, hint, stage);

    let selecting = false;
    let device = savedDevice();
    let agentWidth: number | null = null;
    // Pages visited, for Back and Forward (the frame's own history cannot be
    // read from here).
    const visited: string[] = [];
    let at = -1;
    let moving = 0;

    function layout(): void {
      const preset = DEVICES.find((d) => d.key === device) ?? DEVICES[0];
      const w = agentWidth ?? preset.w;
      const l = frameLayout(stage.clientWidth, stage.clientHeight, w, agentWidth ? null : preset.h);
      Object.assign(frame.style, { width: `${l.width}px`, height: `${l.height}px`, left: `${l.left}px`, top: `${l.top}px`, transform: l.scale === 1 ? '' : `scale(${l.scale})` });
      frame.classList.toggle('framed', !!w && w < 1000);
      size.textContent = w ? `${agentWidth ? 'AI · ' : ''}${w} px${l.scale < 0.995 ? ` · ${Math.round(l.scale * 100)}%` : ''}` : '';
      for (const b of Array.from(devices.children) as HTMLElement[]) b.classList.toggle('on', !agentWidth && b.dataset.device === device);
    }
    for (const d of DEVICES) {
      const b = button('pv-device', d.label, d.w ? `${d.label}: ${d.w} px wide` : 'Fill the panel');
      b.dataset.device = d.key;
      b.addEventListener('click', () => {
        device = d.key;
        agentWidth = null;
        try { localStorage.setItem(DEVICE_KEY, d.key); } catch { /* keep it for now only */ }
        layout();
      });
      devices.appendChild(b);
    }

    function nav(): void {
      backBtn.disabled = at <= 0;
      fwdBtn.disabled = at >= visited.length - 1;
      externalBtn.disabled = !/^(https?:\/\/|localhost|127\.0\.0\.1|\[::1\])/i.test(urlInput.value);
    }
    function arrived(url: string): void {
      if (moving) {
        at = Math.min(Math.max(at + moving, 0), visited.length - 1);
        visited[at] = url;
        moving = 0;
      } else if (visited[at] !== url) {
        visited.splice(at + 1);
        visited.push(url);
        at = visited.length - 1;
      }
      if (document.activeElement !== urlInput) urlInput.value = url;
      nav();
    }

    function load(): void {
      const target = urlInput.value.trim();
      empty.style.display = target ? 'none' : '';
      frame.style.visibility = target ? '' : 'hidden';
      if (!target) return;
      frame.src = toPreviewUrl(target);
      urlInput.value = displayUrl(frame.src);
      arrived(urlInput.value);
    }
    // The agent can try a width (e.g. 390 for a phone); yours comes back when
    // its run ends.
    function setWidth(w: number | null): void {
      agentWidth = w;
      layout();
    }
    current = { frame, urlInput, load, setWidth };

    function postToFrame(msg: unknown): void {
      try { frame.contentWindow?.postMessage(msg, '*'); } catch { /* ignore */ }
    }

    function setSelecting(on: boolean): void {
      selecting = on;
      selectBtn.classList.toggle('active', on);
      hint.textContent = on ? 'Click anything in the page, then say what should change. Click Comment again to cancel.' : '';
      // Post twice to beat any timing race with the injected script's listener.
      postToFrame({ type: 'void:set-selecting', on });
      window.setTimeout(() => postToFrame({ type: 'void:set-selecting', on }), 100);
    }

    // The injected page script posts comments and its address back here.
    const onMessage = (e: MessageEvent): void => {
      if (!frame.isConnected) { window.removeEventListener('message', onMessage); return; }
      if (e.source !== frame.contentWindow) return;
      const d = e.data;
      if (!d || typeof d !== 'object') return;
      if (d.type === 'void:location' && typeof d.url === 'string') { arrived(displayUrl(d.url)); return; }
      if (d.type !== 'void:element-comment') return;
      const c = d.comment as Partial<ElementComment>;
      if (!c || typeof c.comment !== 'string') return;
      bus.emit('preview-comment', {
        id: crypto.randomUUID(),
        comment: c.comment,
        selector: typeof c.selector === 'string' ? c.selector : '',
        tag: typeof c.tag === 'string' ? c.tag : '',
        text: typeof c.text === 'string' ? c.text.replace(/\s+/g, ' ').trim() : '',
        html: typeof c.html === 'string' ? c.html.replace(/\s+/g, ' ').trim() : '',
        pageUrl: typeof c.pageUrl === 'string' ? c.pageUrl : frame.src,
      });
      setSelecting(false);
    };
    window.addEventListener('message', onMessage);
    bus.on('ai-status', ({ streaming }) => {
      if (!streaming && agentWidth && frame.isConnected) setWidth(null);
    });
    new ResizeObserver(() => { if (frame.isConnected) layout(); }).observe(stage);

    frame.addEventListener('load', () => {
      // Re-assert select state after a (re)load so the freshly injected script
      // picks it up.
      postToFrame({ type: 'void:set-selecting', on: selecting });
    });

    backBtn.addEventListener('click', () => { if (at > 0) { moving = -1; postToFrame({ type: 'void:agent', id: 'nav', op: 'back' }); } });
    fwdBtn.addEventListener('click', () => { if (at < visited.length - 1) { moving = 1; postToFrame({ type: 'void:agent', id: 'nav', op: 'forward' }); } });
    reloadBtn.addEventListener('click', load);
    externalBtn.addEventListener('click', () => {
      const v = urlInput.value.trim();
      void window.void.openExternal(/^https?:\/\//i.test(v) ? v : `http://${v}`);
    });
    urlInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') { urlInput.blur(); load(); } });
    urlInput.addEventListener('focus', () => urlInput.select());
    selectBtn.addEventListener('click', () => setSelecting(!selecting));

    layout();
    nav();
    load();
  });
}
