// Ambient background audio: a gapless looping sound played via the Web Audio
// API (sample-accurate loop, no seam at the wrap point). One AudioContext +
// GainNode (volume) + a looping AudioBufferSourceNode (recreated on source
// change, since a buffer source can't be restarted). Decoded buffers are cached
// per-source URL so toggling/volume changes never re-download.

import type { AppSettings } from '../shared/types';

/** Built-in presets, bundled with the app under assets/. */
export interface AmbientPreset {
  id: string;
  label: string;
  file: string; // filename under assets/
}
export const AMBIENT_PRESETS: AmbientPreset[] = [
  { id: 'babft', label: 'BABFT Ambient', file: 'babft-ambient.ogg' },
];

let ctx: AudioContext | null = null;
let gain: GainNode | null = null;
let source: AudioBufferSourceNode | null = null;
let currentUrl: string | null = null; // url currently playing (or loading)
let loadToken = 0; // guards against races when the source changes mid-load
const bufferCache = new Map<string, AudioBuffer>();

/** Resolve the playback URL for the selected source, or null if none. */
function sourceUrl(s: AppSettings): string | null {
  if (s.ambientSource === 'custom') {
    return s.ambientCustomPath ? window.void.mediaUrl(s.ambientCustomPath) : null;
  }
  const preset = AMBIENT_PRESETS.find((p) => p.id === s.ambientSource) ?? AMBIENT_PRESETS[0];
  return preset ? `assets/${preset.file}` : null;
}

function ensureContext(): { ctx: AudioContext; gain: GainNode } {
  if (!ctx || !gain) {
    ctx = new AudioContext();
    gain = ctx.createGain();
    gain.connect(ctx.destination);
  }
  return { ctx, gain };
}

function stopSource(): void {
  if (source) {
    try { source.stop(); } catch { /* already stopped */ }
    try { source.disconnect(); } catch { /* ignore */ }
    source = null;
  }
  currentUrl = null;
}

async function decodeFrom(url: string): Promise<AudioBuffer | null> {
  const cached = bufferCache.get(url);
  if (cached) return cached;
  const { ctx: ac } = ensureContext();
  try {
    const res = await fetch(url);
    if (!res.ok) return null;
    const bytes = await res.arrayBuffer();
    const buf = await ac.decodeAudioData(bytes);
    bufferCache.set(url, buf);
    return buf;
  } catch {
    return null; // network/decode failure — fail quietly, no audio
  }
}

/** Browsers/Electron may start the context suspended until a user gesture.
 *  Resume now; if still blocked, resume once on the next pointer/key event. */
function resumeWhenAllowed(ac: AudioContext): void {
  if (ac.state !== 'suspended') return;
  void ac.resume().catch(() => undefined);
  if (ac.state === 'suspended') {
    const onGesture = (): void => {
      void ac.resume().catch(() => undefined);
      window.removeEventListener('pointerdown', onGesture);
      window.removeEventListener('keydown', onGesture);
    };
    window.addEventListener('pointerdown', onGesture, { once: true });
    window.addEventListener('keydown', onGesture, { once: true });
  }
}

async function startSource(url: string): Promise<void> {
  const token = ++loadToken;
  const buf = await decodeFrom(url);
  if (token !== loadToken) return; // source changed while we were loading
  if (!buf || !ctx || !gain) return;
  stopSource();
  source = ctx.createBufferSource();
  source.buffer = buf;
  source.loop = true; // sample-accurate, gapless
  source.connect(gain);
  source.start();
  currentUrl = url;
  resumeWhenAllowed(ctx);
}

/** Set the volume live (0–1) without an IPC round-trip — for slider dragging.
 *  No-op if audio hasn't been initialized yet. */
export function setAmbientVolume(v: number): void {
  if (gain) gain.gain.value = Math.max(0, Math.min(1, v));
}

/** Single entry point: reconcile playback with the given settings. Called on
 *  boot and on every settings change. */
export function applyAmbient(s: AppSettings): void {
  const url = s.ambientEnabled ? sourceUrl(s) : null;

  if (!url) {
    loadToken++; // cancel any in-flight load
    stopSource();
    return;
  }

  const { ctx: ac, gain: g } = ensureContext();
  // Volume is independent of the source — apply it every time.
  g.gain.value = Math.max(0, Math.min(1, s.ambientVolume));

  if (url !== currentUrl) {
    void startSource(url);
  } else {
    // Same source already playing — just make sure it's audible.
    resumeWhenAllowed(ac);
  }
}
