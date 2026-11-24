// Chat-store helpers for the AI panel controller: token estimation, context
// window lookup, persistence to <project>/.blazeide, and id/title generation.
// Kept separate so controller.ts stays focused on stream/loop control flow.

import type { ModelEntry } from '../../shared/types';
import type { ChatMessage, StoredChat } from '../../shared/types';
import { state } from '../state';

/** Rough token estimate: ~4 chars/token, plus per-message framing and a fixed
 *  allowance for the system prompt + tool schemas the backend prepends (which
 *  aren't in `conversation` but DO count against the context window). Used only
 *  before the first real `usage` report; after that we use the model's actual
 *  prompt_tokens. */
const SYSTEM_OVERHEAD_TOKENS = 1500; // system prompt + tool definitions

export function estimateTokens(messages: ChatMessage[]): number {
  let chars = 0;
  for (const m of messages) {
    chars += (m.content || '').length;
    if (m.toolCalls) {
      for (const c of m.toolCalls) chars += c.name.length + (c.arguments || '').length;
    }
    chars += 12; // role + framing overhead per message
  }
  return Math.ceil(chars / 4) + (messages.length > 0 ? SYSTEM_OVERHEAD_TOKENS : 0);
}

/** Model ids differ a little between BlazeAPI's Normal and Direct lists
 *  (gpt-6-1-sol vs gpt-6.1-sol, payg/ prefix, -trusted suffix); compare them
 *  in one normal form. */
export function normModelId(id: string): string {
  return id.toLowerCase().replace(/^payg\//, '').replace(/-trusted$/, '').replace(/(\d)[-.](\d)/g, '$1.$2');
}

/** Find a model by id, tolerating the Normal/Direct spelling differences. */
export function findModel(id: string): ModelEntry | undefined {
  return state.models.find((x) => x.id === id) ?? state.models.find((x) => x.kind === 'chat' && normModelId(x.id) === normModelId(id));
}

/** The active model's context window (tokens). Falls back to a safe default. */
export function activeContextWindow(): number {
  const DEFAULT = 128_000;
  const m = findModel(state.settings.selectedModel);
  return m && m.contextWindow > 0 ? m.contextWindow : DEFAULT;
}

/** The model name to record on a saved chat. */
export function activeModelLabel(): string {
  const m = findModel(state.settings.selectedModel);
  return m?.displayName ?? state.settings.selectedModel;
}

/** Derive a short title from the first user message. */
export function deriveTitle(messages: ChatMessage[]): string {
  const firstUser = messages.find((m) => m.role === 'user');
  if (!firstUser) return 'New chat';
  // Strip any injected <environment_details> / comment-context preamble so the
  // title reflects what the user actually typed.
  let text = firstUser.content.replace(/<environment_details>[\s\S]*?<\/environment_details>/g, '').trim();
  const firstLine = text.split('\n').find((l) => l.trim().length > 0) ?? text;
  text = firstLine.trim();
  return text.length > 60 ? `${text.slice(0, 60)}…` : text || 'New chat';
}

export function newChatId(): string {
  // randomUUID is available in the renderer (used elsewhere for request ids).
  return crypto.randomUUID();
}

/** Best-effort extraction of a string field's value from possibly-incomplete
 *  JSON (tool arguments still streaming). Returns the decoded value after
 *  `"field":"` even if the closing quote hasn't arrived yet; null if the field
 *  hasn't started. Shared by the live editor-streaming path. */
export function partialJsonField(json: string, field: string): string | null {
  const key = `"${field}"`;
  const at = json.indexOf(key);
  if (at === -1) return null;
  let i = json.indexOf('"', at + key.length); // opening quote of the value
  if (i === -1) return null;
  i += 1;
  let out = '';
  for (; i < json.length; i++) {
    const ch = json[i];
    if (ch === '\\') {
      const next = json[i + 1];
      if (next === undefined) break; // escape split across chunks
      if (next === 'n') out += '\n';
      else if (next === 't') out += '\t';
      else if (next === 'r') out += '\r';
      else out += next;
      i += 1;
      continue;
    }
    if (ch === '"') break;
    out += ch;
  }
  return out;
}

/** Persist a chat to the project's .blazeide folder. No-op without a project or
 *  when the conversation is empty. Returns true if a write happened. */
export async function persistChat(chat: StoredChat): Promise<boolean> {
  const root = state.projectRoot;
  if (!root || chat.messages.length === 0) return false;
  try {
    await window.void.chatSave(root, chat);
    return true;
  } catch {
    return false; // never let a save failure break the conversation
  }
}

/** Build the instruction that asks the model to summarize the conversation so
 *  far into a compact form that preserves task state, decisions, and open
 *  threads — used by the Compact action. */
export const COMPACT_INSTRUCTION =
  'Summarize our conversation so far into a compact briefing that preserves: ' +
  'the task/goal, key decisions made, files created or edited (with their purpose), ' +
  'important code facts discovered, and anything still left to do. ' +
  'Write it as a dense set of notes a fresh assistant could pick up from. ' +
  'Do not include pleasantries or restate this instruction.';
