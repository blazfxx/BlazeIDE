// Central renderer state + typed event bus. Modules communicate through this,
// never by importing each other's internals.

import type {
  AppSettings,
  AIMode,
  AskQuestion,
  ElementComment,
  ModelEntry,
  SessionPublic,
} from '../shared/types';
import { DEFAULT_SETTINGS } from '../shared/types';

export interface BusEvents {
  'project-opened': { root: string; name: string };
  'open-file': { path: string; preserveFocus?: boolean };
  'active-file-changed': { path: string | null };
  'cursor-changed': { line: number; col: number };
  'editor-lang-changed': { lang: string };
  'markers-changed': { errors: number; warnings: number };
  'settings-changed': AppSettings;
  'session-changed': SessionPublic | null;
  'models-changed': ModelEntry[];
  /** An extension was installed or removed: reload themes and snippets. */
  'extensions-changed': void;
  /** The model list could not be loaded (message to show in the picker). */
  'models-error': string;
  'ai-insert-at-cursor': { text: string };
  'ai-apply-edit': { path: string; original: string; modified: string };
  'ai-status': { streaming: boolean };
  /** Put text back into the AI input (after reverting to a message). */
  'ai-prefill': { text: string };
  /** Conversation size changed — drives the context meter. */
  'ai-context': { tokens: number; contextWindow: number };
  /** A saved chat was created/updated/deleted — history list should refresh. */
  'ai-chats-changed': void;
  'ai-mode-changed': { mode: AIMode };
  'git-branch-changed': { branch: string | null };
  'toggle-sidebar': void;
  'sidebar-search': void;
  'toggle-ai-panel': void;
  'toggle-terminal': void;
  /** Delete the file currently selected in the sidebar tree (Cmd+Backspace). */
  'delete-selected-file': void;
  /** Rename the file currently selected in the sidebar tree (Enter / F2). */
  'rename-selected-file': void;
  'open-palette': { mode: 'commands' | 'files' };
  'open-settings': void;
  'open-blbots': void;
  'open-account': void;
  'new-terminal': void;
  'focus-ai-input': void;
  /** Pin files into the AI chat context (from @-mention or tree multi-select). */
  'ai-pin-files': { paths: string[] };
  /** Plan mode finished a plan — switch to Agent and build it. */
  'build-plan': { path: string };
  /** Open the live web preview (defaults to the given file/URL, else the open file). */
  'open-preview': { url?: string };
  /** A user comment on a previewed element, queued for the next AI message. */
  'preview-comment': ElementComment;
  /** The AI asks the user question(s); the input area shows them and resolves. */
  'ai-ask-question': { questions: AskQuestion[]; resolve: (answers: string[]) => void };
}

type Handler<T> = (payload: T) => void;

class EventBus {
  private handlers = new Map<keyof BusEvents, Set<Handler<never>>>();

  on<K extends keyof BusEvents>(event: K, handler: Handler<BusEvents[K]>): () => void {
    let set = this.handlers.get(event);
    if (!set) {
      set = new Set();
      this.handlers.set(event, set);
    }
    set.add(handler as Handler<never>);
    return () => set?.delete(handler as Handler<never>);
  }

  emit<K extends keyof BusEvents>(event: K, payload: BusEvents[K]): void {
    this.handlers.get(event)?.forEach((h) => (h as Handler<BusEvents[K]>)(payload));
  }
}

export const bus = new EventBus();

export interface AppState {
  session: SessionPublic | null;
  settings: AppSettings;
  models: ModelEntry[];
  projectRoot: string | null;
  projectName: string;
}

export const state: AppState = {
  session: null,
  settings: { ...DEFAULT_SETTINGS },
  models: [],
  projectRoot: null,
  projectName: '',
};

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Create an element with class and optional html from trusted template. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls = '',
  html = ''
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (html) e.innerHTML = html;
  return e;
}
