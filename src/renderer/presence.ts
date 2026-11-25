// Keeps the Discord status ("Working on <file> - BlazeCLI - blazeapi.org") in
// step with what is open: the active file, else the project.

import { bus, state } from './state';

let activeFile: string | null = null;

export function currentWork(): string {
  if (activeFile) return activeFile.split(/[\\/]/).pop() ?? activeFile;
  const root = state.projectRoot;
  return root ? root.split(/[\\/]/).filter(Boolean).pop() ?? root : 'a new project';
}

let timer = 0;
function push(): void {
  window.clearTimeout(timer);
  // Small delay: switching tabs quickly sends one update, not ten.
  timer = window.setTimeout(() => void window.void.presenceSet(currentWork()).catch(() => undefined), 600);
}

export function initPresence(): void {
  bus.on('project-opened', () => { activeFile = null; push(); });
  bus.on('active-file-changed', ({ path }) => { activeFile = path; push(); });
  push();
}
