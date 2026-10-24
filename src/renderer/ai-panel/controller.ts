// AI conversation controller: builds requests per mode, routes the stream,
// runs the agent tool loop, applies Edit-mode diffs.

import type { AIStreamEvent, ChatMessage, StoredChat, ToolCall } from '../../shared/types';
import { bus, state } from '../state';
import type { EditorArea } from '../editor/index';
import type { ChatView } from './chat';
import { execToolCall, buildWorkspaceContext } from './toolExec';
import { RunChanges } from './runChanges';
import { joinProjectPath } from '../../shared/applyDiff';
import {
  COMPACT_INSTRUCTION,
  activeContextWindow,
  activeModelLabel,
  deriveTitle,
  estimateTokens,
  newChatId,
  partialJsonField,
  persistChat,
} from './chatStore';

const FILE_CONTEXT_CAP = 24_000;
const AGENT_ROUND_CAP = 8;

export interface Controller {
  /** `userText` is sent to the model; `displayText` (if given) is shown in the
   *  chat instead — used to keep injected context out of the visible bubble. */
  send(userText: string, displayText?: string): Promise<void>;
  cancel(): void;
  isStreaming(): boolean;
  /** Save the current chat (if any) and start a fresh, empty conversation. */
  newChat(): Promise<void>;
  /** Load a saved chat by id into the panel. */
  loadChat(id: string): Promise<void>;
  /** A saved chat was deleted — if it's the one we're editing, forget it so we
   *  don't silently re-save it (which made deletes "come back"). */
  forgetChat(id: string): void;
  /** Replace the conversation with an AI-written summary of it (frees context). */
  compact(): Promise<void>;
}

export function createController(view: ChatView, editor: EditorArea): Controller {
  let conversation: ChatMessage[] = [];
  let activeRequestId: string | null = null;
  let streaming = false;
  // File writes made during the current agent run — drives the changes summary
  // and the "restore to before this run" undo.
  const changes = new RunChanges();
  let cancelled = false; // set by cancel() to break the agent loop mid-run

  // Identity of the chat being persisted to .blazeide. Assigned on the first
  // turn so empty conversations never create a file.
  let chatId: string | null = null;
  let chatTitle = '';
  let chatCreatedAt = 0;

  // Per-request stream accumulation
  let assistantText = '';
  let toolCalls: ToolCall[] = [];
  let resolveStream: ((ev: 'done' | 'error') => void) | null = null;
  // The real prompt-token count the model last reported (its actual context
  // size: system prompt + workspace + history). Authoritative once we have it;
  // before the first response we fall back to a local estimate.
  let lastInputTokens = 0;
  // Live tool-call argument accumulation (per tool index in the current round),
  // used to stream write_file content into the editor as it's generated.
  const liveArgs = new Map<number, string>();
  const liveNames = new Map<number, string>(); // tool name per index
  const liveStreamed = new Set<number>(); // indices we've opened in the editor

  /** Publish the current conversation size so the context meter can update.
   *  Prefer the model's real reported prompt-token count; fall back to a local
   *  estimate (e.g. before the first response, or after editing the chat). */
  function emitContext(): void {
    const estimate = estimateTokens(conversation);
    bus.emit('ai-context', {
      tokens: Math.max(lastInputTokens, estimate),
      contextWindow: activeContextWindow(),
    });
  }

  /** Save the current conversation to .blazeide (if it has any messages). */
  async function save(): Promise<void> {
    if (conversation.length === 0) return;
    if (!chatId) {
      chatId = newChatId();
      chatCreatedAt = Date.now();
    }
    if (!chatTitle) chatTitle = deriveTitle(conversation);
    const chat: StoredChat = {
      id: chatId,
      title: chatTitle,
      createdAt: chatCreatedAt,
      updatedAt: Date.now(),
      mode: state.settings.aiMode,
      model: activeModelLabel(),
      messages: conversation,
    };
    if (await persistChat(chat)) bus.emit('ai-chats-changed', undefined);
  }

  window.void.onAIChunk((ev: AIStreamEvent) => {
    if (ev.requestId !== activeRequestId) return;
    switch (ev.type) {
      case 'text':
        assistantText += ev.text;
        view.appendAssistant(ev.text);
        // In plan/agent mode, after the model's preamble it often goes quiet
        // while generating a (non-streaming) tool argument — keep a working
        // indicator below the text so it never looks frozen.
        if (state.settings.aiMode === 'agent' || state.settings.aiMode === 'plan') {
          view.setWorking(true, 'Working');
        }
        break;
      case 'tool_call':
        toolCalls.push(ev.toolCall);
        break;
      case 'tool_delta': {
        // The model is writing a tool call's arguments (e.g. file content).
        // Show it growing live so agent/edit work never looks frozen.
        view.liveTool(ev.index, ev.name ?? '', ev.argsDelta);
        const acc = (liveArgs.get(ev.index) ?? '') + ev.argsDelta;
        liveArgs.set(ev.index, acc);
        if (ev.name) liveNames.set(ev.index, (liveNames.get(ev.index) ?? '') + ev.name);
        // Stream write_file content straight into the editor (Cursor-style).
        // Only when we'll write directly (agent mode or auto-approve) — in edit
        // mode the diff view is the surface, so we don't touch the live model.
        const liveOk = state.settings.aiMode === 'agent' || state.settings.aiAutoApprove;
        const root = state.projectRoot;
        const isWrite = liveNames.get(ev.index) === 'write_file';
        if (liveOk && root && isWrite) {
          const rel = partialJsonField(acc, 'path');
          const content = partialJsonField(acc, 'content');
          if (rel && content !== null && (content.length > 0 || liveStreamed.has(ev.index))) {
            const joined = joinProjectPath(root, rel);
            if (!('error' in joined)) {
              liveStreamed.add(ev.index);
              void editor.streamInto(joined.abs, content);
            }
          }
        }
        break;
      }
      case 'usage':
        view.addUsage(ev.inputTokens, ev.outputTokens);
        // The reported prompt tokens ARE the real context size — update the
        // meter with the authoritative number. Add output too: by the time the
        // turn is saved, the assistant reply becomes part of the context.
        if (ev.inputTokens > 0) {
          lastInputTokens = ev.inputTokens + ev.outputTokens;
          emitContext();
        }
        break;
      case 'error':
        view.addError(ev.message, ev.status);
        resolveStream?.('error');
        break;
      case 'done':
        resolveStream?.('done');
        break;
    }
  });

  function setStreaming(on: boolean): void {
    streaming = on;
    bus.emit('ai-status', { streaming: on });
  }

  /** Run one model round; resolves when the stream finishes. */
  async function streamRound(): Promise<void> {
    const requestId = crypto.randomUUID();
    activeRequestId = requestId;
    assistantText = '';
    toolCalls = [];
    liveArgs.clear();
    liveNames.clear();
    liveStreamed.clear();
    view.startAssistant();

    const done = new Promise<'done' | 'error'>((resolve) => {
      resolveStream = resolve;
    });

    const active = editor.getActive();
    await window.void.aiSend({
      requestId,
      model: state.settings.selectedModel,
      messages: conversation,
      mode: state.settings.aiMode,
      filename: active ? active.path.split(/[\\/]/).pop() : undefined,
      reasoningEffort: state.settings.reasoningEffort,
      mediaGen: state.settings.aiMediaGen,
    });
    await done;
    resolveStream = null;
    view.finishAssistant();
  }

  async function runAgentLoop(): Promise<void> {
    changes.clear();
    cancelled = false;
    for (let round = 0; round < AGENT_ROUND_CAP; round++) {
      await streamRound();
      if (cancelled) { view.setWorking(false); view.addNotice('Stopped.'); break; }
      if (toolCalls.length === 0) {
        view.setWorking(false);
        if (assistantText) conversation.push({ role: 'assistant', content: assistantText });
        break;
      }
      const calls = toolCalls;
      conversation.push({ role: 'assistant', content: assistantText, toolCalls: calls });
      // The live "writing…" cards have served their purpose; replace them with
      // the real per-call cards as each executes.
      view.clearLiveTools();
      for (const call of calls) {
        if (cancelled) break;
        const result = await execToolCall(call, { editor, view, changes });
        conversation.push({ role: 'tool', content: result, toolCallId: call.id });
      }
      if (cancelled) { view.setWorking(false); view.addNotice('Stopped.'); break; }
      // Tools done for this round; the next round is generating — keep a
      // visible indicator so the panel never looks frozen.
      view.setWorking(true, 'Working');
      if (round === AGENT_ROUND_CAP - 1) {
        view.setWorking(false);
        view.addNotice('Agent stopped after 8 rounds. Send a message to continue.');
      }
    }
    // After the run, show what changed + a one-click restore.
    if (changes.size > 0) {
      view.mountChanges(
        changes.stats(),
        changes.list().map((c) => c.rel),
        (rel) => {
          const root = state.projectRoot;
          if (!root) return;
          const j = joinProjectPath(root, rel);
          if (!('error' in j)) bus.emit('open-file', { path: j.abs });
        },
        () => void restoreRun()
      );
    }
  }

  /** Undo the whole run: rewrite every touched file to its pre-run content, and
   *  delete files that didn't exist before. */
  async function restoreRun(): Promise<void> {
    const items = changes.list();
    if (items.length === 0) return;
    for (const c of items) {
      try {
        if (c.before === null) {
          await window.void.fsDelete(c.abs);
        } else {
          await window.void.fsWriteFile(c.abs, c.before);
          editor.syncOpenFile(c.abs, c.before);
        }
      } catch { /* best-effort per file */ }
    }
    changes.clear();
    view.addNotice(`Restored ${items.length} file${items.length === 1 ? '' : 's'} to before the run.`);
  }

  function buildAskContent(userText: string): string {
    const active = editor.getActive();
    if (!active) return userText;
    const parts: string[] = [userText];
    if (active.selection) {
      parts.push(`\n--- Selected code (${active.path.split('/').pop()}) ---\n${active.selection}`);
    } else {
      const content =
        active.content.length > FILE_CONTEXT_CAP
          ? `${active.content.slice(0, FILE_CONTEXT_CAP)}\n...[truncated]`
          : active.content;
      parts.push(`\n--- Current file: ${active.path.split('/').pop()} ---\n${content}`);
    }
    return parts.join('\n');
  }

  /** Prepend the workspace file tree to the first user turn so the model can
   *  locate files without one being open. */
  async function firstTurnPrefix(): Promise<string> {
    if (conversation.length > 0) return '';
    const ctx = await buildWorkspaceContext();
    return ctx ? `${ctx}\n\n` : '';
  }

  const controller: Controller = {
    async send(userText: string, displayText?: string): Promise<void> {
      if (streaming || !userText.trim()) return;
      const mode = state.settings.aiMode;
      view.addUser(displayText ?? userText);
      setStreaming(true);
      try {
        if (mode === 'agent' || mode === 'plan') {
          // Plan and Agent share one tool-driven loop; they differ only in the
          // tool set and system prompt (resolved in the main process by mode).
          const prefix = await firstTurnPrefix();
          conversation.push({ role: 'user', content: `${prefix}${userText}` });
          await runAgentLoop();
        } else {
          conversation.push({ role: 'user', content: buildAskContent(userText) });
          await streamRound();
          if (assistantText) conversation.push({ role: 'assistant', content: assistantText });
        }
      } finally {
        view.setWorking(false);
        view.clearLiveTools();
        setStreaming(false);
        activeRequestId = null;
        await save();
        emitContext();
      }
    },
    cancel(): void {
      // Stop the current stream AND break the agent loop so it doesn't fire the
      // next round after this one resolves.
      cancelled = true;
      if (activeRequestId) window.void.aiCancel(activeRequestId);
    },
    isStreaming(): boolean {
      return streaming;
    },
    async newChat(): Promise<void> {
      // Stop any in-flight stream first so a late usage/text event can't clobber
      // the fresh chat's context meter.
      if (activeRequestId) { window.void.aiCancel(activeRequestId); activeRequestId = null; }
      if (streaming) setStreaming(false);
      await save();
      conversation = [];
      chatId = null;
      chatTitle = '';
      chatCreatedAt = 0;
      lastInputTokens = 0;
      view.clear();
      emitContext();
    },
    forgetChat(id: string): void {
      // The active chat was deleted from history — detach so we don't re-save
      // it. It becomes a fresh unsaved chat keeping the on-screen messages.
      if (chatId === id) { chatId = null; chatTitle = ''; chatCreatedAt = 0; }
    },
    async loadChat(id: string): Promise<void> {
      if (streaming) return;
      const root = state.projectRoot;
      if (!root) return;
      await save(); // don't lose the current one
      const chat = await window.void.chatLoad(root, id);
      if (!chat) return;
      conversation = chat.messages;
      chatId = chat.id;
      chatTitle = chat.title;
      chatCreatedAt = chat.createdAt;
      lastInputTokens = 0; // unknown until the next response; estimate for now
      view.replay(conversation);
      emitContext();
    },
    async compact(): Promise<void> {
      if (streaming || conversation.length === 0) return;
      setStreaming(true);
      view.addNotice('Compacting conversation…');
      try {
        // Ask the model to summarize, capturing the summary text without
        // persisting the request/summary turns into the kept history.
        conversation.push({ role: 'user', content: COMPACT_INSTRUCTION });
        await streamRound();
        const summary = assistantText.trim();
        if (summary) {
          conversation = [
            { role: 'assistant', content: `**Summary of earlier conversation:**\n\n${summary}` },
          ];
          lastInputTokens = 0; // context shrank; re-estimate until next response
          view.replay(conversation);
          view.addNotice('Conversation compacted.');
        } else {
          conversation.pop(); // drop the compact request; nothing came back
          view.addError('Compaction produced no summary.');
        }
      } finally {
        setStreaming(false);
        activeRequestId = null;
        await save();
        emitContext();
      }
    },
  };

  // Plan → Agent handoff: switch to Agent mode and build the saved plan.
  bus.on('build-plan', ({ path }) => {
    if (streaming) return;
    if (state.settings.aiMode !== 'agent') {
      state.settings.aiMode = 'agent';
      void window.void.settingsSet({ aiMode: 'agent' });
      bus.emit('ai-mode-changed', { mode: 'agent' });
    }
    void controller.send(
      `Implement the plan saved at ${path}. Read it first, then build it step by step.`,
      'Implementing the plan ↑'
    );
  });

  return controller;
}
