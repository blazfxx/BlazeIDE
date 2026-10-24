import type { VoidAPI } from '../shared/ipc';

declare global {
  interface Window {
    void: VoidAPI;
  }
}

export {};
