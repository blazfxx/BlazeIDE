// Applies visual settings (theme, custom background image/video) to the
// document. Called on boot and whenever settings change so the workbench
// reflects them live.

import type { AppSettings } from '../shared/types';

const VIDEO_ID = 'bg-video';

export function applyAppearance(s: AppSettings): void {
  const root = document.documentElement;
  root.dataset.theme = s.theme === 'light' ? 'light' : 'dark';

  const body = document.body;

  // Shared placement/look vars used by both the image layer and the video.
  root.style.setProperty('--bg-blur', `${Math.max(0, s.backgroundBlur)}px`);
  root.style.setProperty('--bg-opacity', String(Math.min(1, Math.max(0, s.backgroundOpacity))));
  root.style.setProperty('--bg-pos-x', `${clampPct(s.backgroundPosX)}%`);
  root.style.setProperty('--bg-pos-y', `${clampPct(s.backgroundPosY)}%`);
  root.style.setProperty('--bg-scale', String(Math.max(0.1, s.backgroundScale / 100)));
  root.style.setProperty('--bg-fit', s.backgroundFit === 'contain' ? 'contain' : 'cover');
  root.style.setProperty('--bg-rotate', `${s.backgroundRotation || 0}deg`);
  // Sharpen/enhance: crisp pixel scaling plus a contrast/saturation lift. The
  // amount (0–100) scales how strong the boost is.
  if (s.backgroundSharpen) {
    const a = Math.min(100, Math.max(0, s.backgroundSharpenAmount)) / 100;
    const contrast = (1 + 0.35 * a).toFixed(3);
    const saturate = (1 + 0.4 * a).toFixed(3);
    const brightness = (1 + 0.05 * a).toFixed(3);
    root.style.setProperty('--bg-enhance', `contrast(${contrast}) saturate(${saturate}) brightness(${brightness})`);
    root.style.setProperty('--bg-rendering', 'crisp-edges');
  } else {
    root.style.setProperty('--bg-enhance', 'none');
    root.style.setProperty('--bg-rendering', 'auto');
  }

  const hasVideo = Boolean(s.backgroundVideo);
  const hasImage = !hasVideo && Boolean(s.backgroundImage);

  // Image layer (body::before) — only when no video is set.
  if (hasImage && s.backgroundImage) {
    const safe = s.backgroundImage.replace(/"/g, '\\"');
    root.style.setProperty('--bg-image', `url("${safe}")`);
  } else {
    root.style.setProperty('--bg-image', 'none');
  }

  // Video layer — a real <video> element, created lazily and reused.
  applyVideo(hasVideo ? s.backgroundVideo : null);

  if (hasVideo || hasImage) {
    body.dataset.hasBg = '1';
  } else {
    root.style.setProperty('--bg-opacity', '0');
    delete body.dataset.hasBg;
  }

  // Auto-contrast: keep text legible over the background. Enabled only when a
  // background is active; otherwise clear so the theme's own colours apply.
  if (s.backgroundAutoContrast && (hasImage || hasVideo)) {
    setContrast();
  } else {
    clearAutoContrast();
  }
}

// Auto-contrast keeps text legible over a custom background. We do NOT flip to
// dark text: most text sits on dark panels, so dark text there is invisible
// (black-on-black). Instead we keep readable light text everywhere and add a
// single soft dark halo behind it — that reads on both light and dark images.
function setContrast(): void {
  const root = document.documentElement;
  root.style.setProperty('--text-primary', '#fafafa');
  root.style.setProperty('--text-secondary', '#cdcdcd');
  root.style.setProperty('--text-halo', 'rgba(0,0,0,0.7)');
  document.body.dataset.autoContrast = 'on';
}

function clearAutoContrast(): void {
  const root = document.documentElement;
  root.style.removeProperty('--text-primary');
  root.style.removeProperty('--text-secondary');
  root.style.removeProperty('--text-halo');
  delete document.body.dataset.autoContrast;
}

// The background video is decoded by a <video> that is never in the page and
// painted onto a canvas. WebKit (macOS) draws its own play button on any
// <video> it lays out and thinks is paused; a detached element has no such
// overlay, and a canvas has nothing to overlay.
let bgVideo: HTMLVideoElement | null = null;
let bgFrame = 0;

function stopVideo(): void {
  cancelAnimationFrame(bgFrame);
  bgFrame = 0;
  if (bgVideo) {
    bgVideo.pause();
    bgVideo.removeAttribute('src');
    bgVideo.load();
    bgVideo = null;
  }
  document.getElementById(VIDEO_ID)?.remove();
}

function applyVideo(src: string | null): void {
  if (!src) {
    stopVideo();
    return;
  }
  // Served by the sandboxed blazemedia: protocol (only picked files and files
  // inside an open project are readable).
  const url = window.void.mediaUrl(src);
  let canvas = document.getElementById(VIDEO_ID) as HTMLCanvasElement | null;
  if (!canvas || !bgVideo) {
    stopVideo();
    canvas = document.createElement('canvas');
    canvas.id = VIDEO_ID;
    canvas.setAttribute('aria-hidden', 'true');
    // Insert as the first child of <body> so it sits behind #app (z-index 0).
    document.body.insertBefore(canvas, document.body.firstChild);
    const video = document.createElement('video');
    video.muted = true;
    video.loop = true;
    video.autoplay = true;
    video.playsInline = true;
    video.setAttribute('muted', '');
    video.setAttribute('playsinline', '');
    video.disablePictureInPicture = true;
    video.disableRemotePlayback = true;
    const kick = (): void => { if (video.paused) void video.play().catch(() => {}); };
    video.addEventListener('loadeddata', kick);
    video.addEventListener('canplay', kick);
    video.addEventListener('pause', () => setTimeout(kick, 0));
    document.addEventListener('visibilitychange', () => { if (!document.hidden) kick(); });
    bgVideo = video;
    const ctx = canvas.getContext('2d');
    const draw = (): void => {
      bgFrame = requestAnimationFrame(draw);
      const c = canvas as HTMLCanvasElement;
      if (!ctx || !bgVideo || bgVideo.readyState < 2 || bgVideo.videoWidth === 0) return;
      const k = Math.min(1, 1920 / bgVideo.videoWidth);
      const w = Math.round(bgVideo.videoWidth * k);
      const h = Math.round(bgVideo.videoHeight * k);
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      ctx.drawImage(bgVideo, 0, 0, w, h);
    };
    bgFrame = requestAnimationFrame(draw);
  }
  if (bgVideo && bgVideo.getAttribute('data-src') !== url) {
    bgVideo.setAttribute('data-src', url);
    bgVideo.src = url;
    void bgVideo.play().catch(() => {
      /* retried on loadeddata/canplay */
    });
  }
}

function clampPct(n: number): number {
  if (!Number.isFinite(n)) return 50;
  return Math.min(100, Math.max(0, n));
}
