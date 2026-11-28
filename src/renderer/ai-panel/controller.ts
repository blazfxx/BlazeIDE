// AI conversation controller: builds requests per mode, routes the stream,
// runs the agent tool loop, applies Edit-mode diffs.

import type { AIStreamEvent, ChatMessage, Checkpoint, StoredChat, ToolCall } from '../../shared/types';
import { bus, el, state } from '../state';
import type { EditorArea } from '../editor/index';
import type { ChatView } from './chat';
import { execToolCall, buildWorkspaceContext } from './toolExec';
import { RunChanges } from './runChanges';
import { approvalLevel } from './approval';
import { joinProjectPath } from '../../shared/applyDiff';
import {
  COMPACT_INSTRUCTION,
  findModel,
  cannotSee,
  activeContextWindow,
  activeModelLabel,
  deriveTitle,
  estimateTokens,
  newChatId,
  partialJsonField,
  persistChat,
} from './chatStore';

const FILE_CONTEXT_CAP = 24_000;
// One round = one model call (it may run several tools). Building a small site
// takes 20-40 rounds, so 8 used to stop agents in the middle of the work.
const AGENT_ROUND_CAP = 60;
// A round that fails with a temporary error (timeout, 5xx, rate limit, a
// dropped stream) is sent again this many times before the run stops.
const ROUND_RETRIES = 3;
const CONTINUE_PROMPT = 'Continue where you left off. Check what is already done first, then finish the task.';

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
/** Errors worth retrying: no status (network/stream), 408/409/425/429, 5xx. */
function retryable(status?: number): boolean {
  return status === undefined || status === 408 || status === 409 || status === 425 || status === 429 || status >= 500;
}

export interface Controller {
  /** `userText` is sent to the model; `displayText` (if given) is shown in the
   *  chat instead — used to keep injected context out of the visible bubble. */
  send(userText: string, displayText?: string, images?: string[]): Promise<void>;
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
  // Per user turn: the files the agent changed after it (earlier content), for "Revert".
  let checkpoints: Checkpoint[] = [];
  const CHECKPOINT_BUDGET = 4_000_000; // characters kept in the saved chat; oldest dropped first
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
  // updatedAt of the copy on screen; a newer file on disk was written by the
  // blaze CLI or a phone (Continue on mobile) and is loaded instead.
  let knownUpdatedAt = 0;

  // Per-request stream accumulation
  let assistantText = '';
  let assistantReasoning = '';
  let lastError: { message: string; status?: number } | null = null;
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

  // While a run is going, only what is complete is saved: a tool call without
  // its result could not be continued from.
  let stableLength = 0;
  let saveWarned = false;
  const HEARTBEAT_MS = 20_000;
  setInterval(() => { if (streaming) void save(); }, HEARTBEAT_MS);

  /** Mark everything so far as complete and save it, so a phone sees it now. */
  function checkpointSave(): void {
    stableLength = conversation.length;
    void save();
  }

  // One save at a time, in order: a late "still working" save must not land
  // after the final one.
  let saving: Promise<void> = Promise.resolve();
  function save(): Promise<void> {
    saving = saving.then(writeChat, writeChat);
    return saving;
  }

  /** Save the current conversation to .blazeide (if it has any messages). */
  async function writeChat(): Promise<void> {
    if (conversation.length === 0) return;
    if (!chatId) {
      chatId = newChatId();
      chatCreatedAt = Date.now();
    }
    if (!chatTitle) chatTitle = deriveTitle(conversation);
    knownUpdatedAt = Date.now();
    const chat: StoredChat = {
      id: chatId,
      title: chatTitle,
      createdAt: chatCreatedAt,
      updatedAt: knownUpdatedAt,
      mode: state.settings.aiMode,
      model: activeModelLabel(),
      messages: streaming ? conversation.slice(0, stableLength) : conversation,
      checkpoints,
      ...(streaming ? { live: knownUpdatedAt } : {}),
    };
    if (chat.messages.length === 0) return;
    if (await persistChat(chat)) {
      bus.emit('ai-chats-changed', undefined);
    } else if (state.projectRoot && !saveWarned) {
      saveWarned = true;
      view.addNotice('This chat could not be saved, so it will not show up in chat history or on your phone.');
    }
  }

  /** Load the open chat again if another app saved a newer copy. Never while
   *  this panel is working, and never by saving first (that would overwrite
   *  the newer copy). */
  async function refreshFromDisk(): Promise<void> {
    const root = state.projectRoot;
    const id = chatId;
    if (streaming || !root || !id) return;
    const chat = await window.void.chatLoad(root, id).catch(() => null);
    if (!chat || streaming || chatId !== id || (chat.updatedAt ?? 0) <= knownUpdatedAt) return;
    conversation = chat.messages;
    checkpoints = chat.checkpoints ?? [];
    chatTitle = chat.title;
    knownUpdatedAt = chat.updatedAt ?? 0;
    view.replay(conversation, (i) => void revertTo(i));
    view.addNotice('Updated with messages sent from the blaze CLI or your phone.');
    emitContext();
  }
  window.addEventListener('focus', () => void refreshFromDisk());
  document.addEventListener('visibilitychange', () => { if (!document.hidden) void refreshFromDisk(); });
  setInterval(() => { if (!document.hidden) void refreshFromDisk(); }, 15_000);

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
      case 'reasoning':
        // Kept for the next round (thinking models need it back), not shown.
        assistantReasoning += ev.text;
        if (!assistantText) view.setWorking(true, 'Thinking');
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
        const liveOk = approvalLevel() !== 'ask';
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
        // Shown by the caller, which may retry the round first.
        lastError = { message: ev.message, status: ev.status };
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

  function showLastError(): void {
    if (lastError) view.addError(lastError.message, lastError.status);
    lastError = null;
  }

  /** A button that resumes the agent where it stopped. */
  function offerContinue(): void {
    const b = el('button', 'chat-continue', 'Continue');
    b.addEventListener('click', () => {
      if (streaming) return;
      b.remove();
      void controller.send(CONTINUE_PROMPT, 'Continue');
    });
    view.mount(b);
  }

  /** Run one model round; resolves when the stream finishes. */
  async function streamRound(): Promise<'done' | 'error'> {
    const requestId = crypto.randomUUID();
    activeRequestId = requestId;
    assistantText = '';
    assistantReasoning = '';
    lastError = null;
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
    const result = await done;
    resolveStream = null;
    view.finishAssistant();
    return result;
  }

  async function runAgentLoop(): Promise<void> {
    changes.clear();
    cancelled = false;
    let retries = 0;
    for (let round = 0; round < AGENT_ROUND_CAP; round++) {
      const result = await streamRound();
      if (cancelled) { view.setWorking(false); view.addNotice('Stopped.'); break; }
      if (result === 'error' && toolCalls.length === 0) {
        const e = lastError;
        if (e && retryable(e.status) && retries < ROUND_RETRIES) {
          // Temporary failure: the conversation is intact, so just send the
          // same round again instead of ending the whole run.
          retries++;
          view.addNotice(`${e.message} Retrying (${retries}/${ROUND_RETRIES})…`);
          lastError = null;
          view.setWorking(true, 'Retrying');
          await sleep(e.status === 429 ? 5000 * retries : 1500 * retries);
          if (cancelled) { view.setWorking(false); view.addNotice('Stopped.'); break; }
          round--;
          continue;
        }
        view.setWorking(false);
        showLastError();
        if (conversation.some((m) => m.role === 'tool')) offerContinue();
        break;
      }
      retries = 0;
      if (toolCalls.length === 0) {
        view.setWorking(false);
        if (assistantText) conversation.push({ role: 'assistant', content: assistantText });
        break;
      }
      const calls = toolCalls;
      conversation.push({ role: 'assistant', content: assistantText, toolCalls: calls, ...(assistantReasoning ? { reasoning: assistantReasoning } : {}) });
      const shots: Array<{ label: string; url: string }> = [];
      // The live "writing…" cards have served their purpose; replace them with
      // the real per-call cards as each executes.
      view.clearLiveTools();
      for (const call of calls) {
        if (cancelled) break;
        const result = await execToolCall(call, { editor, view, changes, shots });
        conversation.push({ role: 'tool', content: result, toolCallId: call.id });
      }
      // Browser screenshots go to the model as images on a user turn (tool
      // results cannot carry images on most providers).
      if (shots.length > 0) {
        const m = findModel(state.settings.selectedModel);
        const blind = !!m && cannotSee(m);
        conversation.push(blind
          ? { role: 'user', content: `[${shots.length} browser screenshot(s) were taken, but ${m?.displayName ?? 'this model'} cannot see images. Use the console action and the code to check the page instead.]` }
          : { role: 'user', content: `[Browser screenshot(s) you just took: ${shots.map((s) => s.label).join('; ')}. Look at them carefully and fix what is wrong before finishing.]`, images: shots.map((s) => s.url) });
      }
      // The round is complete: calls and their results.
      checkpointSave();
      if (cancelled) { view.setWorking(false); view.addNotice('Stopped.'); break; }
      // Tools done for this round; the next round is generating — keep a
      // visible indicator so the panel never looks frozen.
      view.setWorking(true, 'Working');
      if (round === AGENT_ROUND_CAP - 1) {
        view.setWorking(false);
        view.addNotice(`Paused after ${AGENT_ROUND_CAP} steps so it never runs away with your balance.`);
        offerContinue();
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

  function trimCheckpoints(): void {
    let total = 0;
    for (let i = checkpoints.length - 1; i >= 0; i--) {
      total += checkpoints[i].files.reduce((n, f) => n + (f.before?.length ?? 0) + f.rel.length, 0);
      if (total > CHECKPOINT_BUDGET) { checkpoints = checkpoints.slice(i + 1); break; }
    }
  }

  /** Revert to the user message at `userIndex`: put every file the agent changed
   *  in that turn or later back as it was before it, drop that message and
   *  everything after it, and give its text back to the input to edit. */
  async function revertTo(userIndex: number): Promise<void> {
    if (streaming) return;
    const root = state.projectRoot;
    const msg = conversation[userIndex];
    if (!msg || msg.role !== 'user') return;
    const later = checkpoints.filter((c) => c.userIndex >= userIndex).sort((a, b) => b.userIndex - a.userIndex);
    let restored = 0, failed = 0;
    if (root) {
      // Newest turn first, so a file touched in several turns ends at its oldest "before".
      for (const cp of later) {
        for (const f of cp.files) {
          const j = joinProjectPath(root, f.rel);
          if ('error' in j) { failed++; continue; }
          try {
            if (f.before === null) await window.void.fsDelete(j.abs);
            else { await window.void.fsWriteFile(j.abs, f.before); editor.syncOpenFile(j.abs, f.before); }
            restored++;
          } catch { failed++; }
        }
      }
    }
    const files = new Set(later.flatMap((c) => c.files.map((f) => f.rel)));
    checkpoints = checkpoints.filter((c) => c.userIndex < userIndex);
    conversation = conversation.slice(0, userIndex);
    lastInputTokens = 0;
    view.replay(conversation, (i) => void revertTo(i));
    view.addNotice(files.size
      ? `Reverted ${files.size} file${files.size === 1 ? '' : 's'} to before that message${failed ? ` (${failed} could not be restored)` : ''}. Edit the message and send it again.`
      : 'Went back to that message (the AI had not changed any files after it).');
    void restored;
    const text = msg.content.replace(/<environment_details>[\s\S]*?<\/environment_details>/g, '').trim();
    bus.emit('ai-prefill', { text });
    if (conversation.length === 0 && chatId && root) {
      // Reverted to the very first message: the saved chat would be empty, so
      // remove it (save() skips empty chats, which left the old one on disk).
      await window.void.chatDelete(root, chatId).catch(() => undefined);
      chatId = null; chatTitle = ''; chatCreatedAt = 0;
      bus.emit('ai-chats-changed', undefined);
    } else {
      await save();
    }
    emitContext();
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
    async send(userText: string, displayText?: string, images?: string[]): Promise<void> {
      const imgs = (images ?? []).filter(Boolean);
      if (streaming || (!userText.trim() && imgs.length === 0)) return;
      if (!userText.trim()) userText = 'Look at the attached image(s).';
      const mode = state.settings.aiMode;
      const turnIndex = conversation.length;
      view.addUser(displayText ?? userText, imgs, () => void revertTo(turnIndex));
      const withImages = imgs.length ? { images: imgs } : {};
      setStreaming(true);
      try {
        if (mode === 'agent' || mode === 'plan') {
          // Plan and Agent share one tool-driven loop; they differ only in the
          // tool set and system prompt (resolved in the main process by mode).
          const prefix = await firstTurnPrefix();
          conversation.push({ role: 'user', content: `${prefix}${userText}`, ...withImages });
          if (!chatTitle) chatTitle = deriveTitle([{ role: 'user', content: userText }]);
          checkpointSave();
          try {
            await runAgentLoop();
          } finally {
            if (changes.size > 0) {
              checkpoints.push({ userIndex: turnIndex, files: changes.list().map((c) => ({ rel: c.rel, before: c.before })) });
              trimCheckpoints();
            }
          }
        } else {
          conversation.push({ role: 'user', content: buildAskContent(userText), ...withImages });
          if (!chatTitle) chatTitle = deriveTitle([{ role: 'user', content: userText }]);
          checkpointSave();
          if ((await streamRound()) === 'error') showLastError();
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
      checkpoints = [];
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
      checkpoints = chat.checkpoints ?? [];
      chatId = chat.id;
      chatTitle = chat.title;
      chatCreatedAt = chat.createdAt;
      knownUpdatedAt = chat.updatedAt ?? 0;
      lastInputTokens = 0; // unknown until the next response; estimate for now
      view.replay(conversation, (i) => void revertTo(i));
      emitContext();
    },
    async compact(): Promise<void> {
      if (streaming || conversation.length === 0) return;
      setStreaming(true);
      view.addNotice('Compacting conversation…');
      try {
        // Ask the model to summarize, capturing the summary text without
        // persisting the request/summary turns into the kept history.
        stableLength = conversation.length;
        conversation.push({ role: 'user', content: COMPACT_INSTRUCTION });
        if ((await streamRound()) === 'error') showLastError();
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
