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

function toPreviewUrl(input: string): string {
  const v = input.trim();
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
    host.style.cssText = 'display:flex;flex-direction:column;height:100%;position:relative;';

    const bar = el('div', 'preview-bar');
    const urlInput = el('input', 'preview-url') as HTMLInputElement;
    urlInput.spellcheck = false;
    urlInput.placeholder = 'file path or http://localhost:3000';
    urlInput.value = start;
    const reloadBtn = el('button', 'preview-btn', '⟳');
    reloadBtn.title = 'Reload';
    const selectBtn = el('button', 'preview-btn preview-select', icon(iconAgent, 'icon-12'));
    selectBtn.title = 'Select an element to comment on';
    const hint = el('span', 'preview-hint');
    bar.append(urlInput, reloadBtn, selectBtn, hint);

    const frame = el('iframe', 'preview-frame') as HTMLIFrameElement;
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-forms allow-modals');
    host.append(bar, frame);

    let selecting = false;

    function load(): void {
      frame.src = toPreviewUrl(urlInput.value);
    }
    // The agent can emulate a viewport width (e.g. 390 for a phone).
    function setWidth(w: number | null): void {
      frame.style.width = w ? `${w}px` : '';
      frame.style.maxWidth = w ? 'none' : '';
      frame.style.alignSelf = w ? 'center' : '';
      frame.style.flex = w ? 'none' : '';
      frame.style.height = w ? '100%' : '';
    }
    current = { frame, urlInput, load, setWidth };

    function postToFrame(msg: unknown): void {
      try { frame.contentWindow?.postMessage(msg, '*'); } catch { /* ignore */ }
    }

    function setSelecting(on: boolean): void {
      selecting = on;
      selectBtn.classList.toggle('active', on);
      hint.textContent = on ? 'Click an element in the page to comment' : '';
      // Post twice to beat any timing race with the injected script's listener.
      postToFrame({ type: 'void:set-selecting', on });
      window.setTimeout(() => postToFrame({ type: 'void:set-selecting', on }), 100);
    }

    // The injected page script posts comments back here.
    const onMessage = (e: MessageEvent): void => {
      if (e.source !== frame.contentWindow) return;
      const d = e.data;
      if (!d || typeof d !== 'object' || d.type !== 'void:element-comment') return;
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

    frame.addEventListener('load', () => {
      // Re-assert select state after a (re)load so the freshly injected script
      // picks it up.
      postToFrame({ type: 'void:set-selecting', on: selecting });
    });

    reloadBtn.addEventListener('click', load);
    urlInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') load(); });
    selectBtn.addEventListener('click', () => setSelecting(!selecting));

    if (start) load();
  });
}
