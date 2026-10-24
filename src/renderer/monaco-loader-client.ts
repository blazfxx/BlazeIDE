// Lazy Monaco access for renderer modules. Never import 'monaco-editor'
// directly anywhere else — always go through loadMonaco().
import type { monaco as MonacoNS } from './monaco-loader';

export type Monaco = typeof MonacoNS;

let cached: Promise<Monaco> | null = null;

function ensureCss(): void {
  if (document.querySelector('link[data-monaco]')) return;
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = './monaco.css';
  link.setAttribute('data-monaco', '1');
  document.head.appendChild(link);
}

export function loadMonaco(): Promise<Monaco> {
  if (!cached) {
    ensureCss();
    cached = import('./monaco.js').then((m: { monaco: Monaco }) => m.monaco);
  }
  return cached;
}
