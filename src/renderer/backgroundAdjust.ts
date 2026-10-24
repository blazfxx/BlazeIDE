// Interactive "adjust background" mode: drag the wallpaper to reposition and
// scroll/pinch to zoom, then Done to persist. Live-updates the same CSS vars
// appearance.ts uses, so the preview is the real workbench background.

import { el, state } from './state';
import { apply } from './settings/controls';

const MIN_SCALE = 20;
const MAX_SCALE = 400;

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

/** Open the drag-to-adjust overlay. Resolves when the user clicks Done/Cancel. */
export function openBackgroundAdjust(): void {
  if (!state.settings.backgroundImage && !state.settings.backgroundVideo) return;
  const overlay = document.getElementById('overlay-root');
  if (!overlay) return;

  const root = document.documentElement;
  // Working copy of the adjustable values.
  let posX = state.settings.backgroundPosX;
  let posY = state.settings.backgroundPosY;
  let scale = state.settings.backgroundScale;
  let rotation = state.settings.backgroundRotation || 0;

  const startX = posX;
  const startY = posY;
  const startScale = scale;
  const startRotation = rotation;

  function paint(): void {
    root.style.setProperty('--bg-pos-x', `${posX}%`);
    root.style.setProperty('--bg-pos-y', `${posY}%`);
    root.style.setProperty('--bg-scale', String(scale / 100));
    root.style.setProperty('--bg-rotate', `${rotation}deg`);
  }

  const layer = el('div', 'bg-adjust');
  const hint = el('div', 'bg-adjust-hint', 'Drag to move · scroll to zoom · Shift+scroll to rotate');
  const bar = el('div', 'bg-adjust-bar');
  const doneBtn = el('button', 'primary', 'Done');
  const cancelBtn = el('button', '', 'Cancel');
  bar.append(hint, cancelBtn, doneBtn);
  layer.append(bar);
  overlay.appendChild(layer);
  // Fade the workbench chrome so the wallpaper reads clearly while adjusting.
  const appEl = document.getElementById('app');
  if (appEl) appEl.classList.add('bg-adjusting');

  // --- drag to pan ---
  let dragging = false;
  let lastX = 0;
  let lastY = 0;
  layer.addEventListener('pointerdown', (e: PointerEvent) => {
    if ((e.target as HTMLElement).closest('.bg-adjust-bar')) return;
    dragging = true;
    lastX = e.clientX;
    lastY = e.clientY;
    layer.setPointerCapture(e.pointerId);
    layer.classList.add('grabbing');
  });
  layer.addEventListener('pointermove', (e: PointerEvent) => {
    if (!dragging) return;
    // Convert pixel drag into a percentage shift. Dragging right reveals the
    // left of the image, i.e. the focal point moves left → subtract.
    const w = window.innerWidth || 1;
    const h = window.innerHeight || 1;
    posX = clamp(posX - ((e.clientX - lastX) / w) * 100, 0, 100);
    posY = clamp(posY - ((e.clientY - lastY) / h) * 100, 0, 100);
    lastX = e.clientX;
    lastY = e.clientY;
    paint();
  });
  const endDrag = (e: PointerEvent): void => {
    if (!dragging) return;
    dragging = false;
    try { layer.releasePointerCapture(e.pointerId); } catch { /* ignore */ }
    layer.classList.remove('grabbing');
  };
  layer.addEventListener('pointerup', endDrag);
  layer.addEventListener('pointercancel', endDrag);

  // --- scroll to zoom; Shift+scroll to rotate ---
  layer.addEventListener(
    'wheel',
    (e: WheelEvent) => {
      e.preventDefault();
      if (e.shiftKey) {
        rotation = (rotation - e.deltaY * 0.25) % 360;
      } else {
        scale = clamp(scale - e.deltaY * 0.25, MIN_SCALE, MAX_SCALE);
      }
      paint();
    },
    { passive: false }
  );

  function close(): void {
    layer.remove();
    if (appEl) appEl.classList.remove('bg-adjusting');
  }

  doneBtn.addEventListener('click', () => {
    close();
    void apply({ backgroundPosX: Math.round(posX), backgroundPosY: Math.round(posY), backgroundScale: Math.round(scale), backgroundRotation: Math.round(rotation) });
  });
  cancelBtn.addEventListener('click', () => {
    // Revert the live preview to where we started.
    posX = startX; posY = startY; scale = startScale; rotation = startRotation;
    paint();
    close();
  });

  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') {
      cancelBtn.click();
      window.removeEventListener('keydown', onKey, true);
    } else if (e.key === 'Enter') {
      doneBtn.click();
      window.removeEventListener('keydown', onKey, true);
    }
  };
  window.addEventListener('keydown', onKey, true);

  paint();
}
