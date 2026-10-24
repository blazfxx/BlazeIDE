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

function toPreviewUrl(input: string): string {
  const v = input.trim();
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

export function initPreview(editor: EditorArea): void {
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
