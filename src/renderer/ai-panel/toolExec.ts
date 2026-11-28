// Tool-call execution for the AI panel, shared by Edit and Agent modes.
//
// Non-mutating tools (read/list/search) run in the main process via aiRunTool.
// run_command keeps its Approve/Reject card. Mutating tools (apply_diff,
// write_file) are gated by the editor diff view instead: the diff IS the
// approval surface. When auto-approve is on, mutating tools write directly.

import type { AskQuestion, ToolCall } from '../../shared/types';
import type { FileNode } from '../../shared/types';
import { applyDiff, joinProjectPath, SKIP_DIRS } from '../../shared/applyDiff';
import { bus, state } from '../state';
import type { EditorArea } from '../editor/index';
import type { ChatView } from './chat';
import type { RunChanges } from './runChanges';
import { createToolCard, type ToolCard } from './agentView';
import { approvalLevel, mustAsk } from './approval';
import { agentBrowser } from '../preview/index';

/** Show the AI's question(s) in the input area and wait for the user's answers,
 *  returning them as the tool result for the model. */
function execAskQuestion(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  const card = createToolCard(call, { gated: false });
  deps.view.mount(card.element);
  card.setRunning();

  let questions: AskQuestion[] = [];
  try {
    const parsed: unknown = JSON.parse(call.arguments || '{}');
    const q = (parsed as { questions?: unknown }).questions;
    if (Array.isArray(q)) {
      questions = q
        .filter((x): x is { question: string; options?: unknown } => !!x && typeof (x as { question?: unknown }).question === 'string')
        .map((x) => ({ question: x.question, options: Array.isArray(x.options) ? x.options.filter((o): o is string => typeof o === 'string') : undefined }));
    }
  } catch {
    return Promise.resolve('Error: malformed ask_question arguments.');
  }
  if (questions.length === 0) return Promise.resolve('Error: ask_question needs at least one question.');

  return new Promise<string>((resolve) => {
    bus.emit('ai-ask-question', {
      questions,
      resolve: (answers: string[]) => {
        const result = questions
          .map((q, i) => `Q: ${q.question}\nA: ${answers[i] ?? '(no answer)'}`)
          .join('\n\n');
        card.setResult(result, false);
        resolve(result);
      },
    });
  });
}

const MUTATING = new Set<string>(['apply_diff', 'write_file']);
const TREE_CONTEXT_CAP = 24_000;

export interface ToolExecDeps {
  editor: EditorArea;
  view: ChatView;
  /** Screenshots taken this round; the controller attaches them to the next request. */
  shots?: Array<{ label: string; url: string }>;
  /** Accumulates this run's file writes for the changes summary + undo. */
  changes: RunChanges;
}

function parseArgs(raw: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(raw || '{}');
    return parsed !== null && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {};
  } catch {
    return null;
  }
}

/** Generate an image via the backend, save it into the project, and open it so
 *  the user sees the result. */
async function execGenerateMedia(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  const card = createToolCard(call, { gated: false });
  deps.view.mount(card.element);
  card.setRunning();

  const root = state.projectRoot;
  if (!root) { card.setResult('Error: no project folder is open.', true); return 'Error: no project folder is open.'; }

  const args = parseArgs(call.arguments);
  if (!args) { card.setResult('Error: invalid tool arguments.', true); return 'Error: tool arguments were not valid JSON.'; }
  const prompt = typeof args.prompt === 'string' ? args.prompt : '';
  const rel = typeof args.path === 'string' ? args.path : '';
  const aspectRatio = typeof args.aspect_ratio === 'string' ? args.aspect_ratio : undefined;
  if (!prompt || !rel) {
    const msg = 'Error: generate needs both a prompt and a path.';
    card.setResult(msg, true);
    return msg;
  }

  const result = await window.void.aiGenerateMedia({ kind: 'image', prompt, path: rel, aspectRatio, projectRoot: root });
  const isErr = result.startsWith('Error:');
  card.setResult(result, isErr);
  if (!isErr) {
    const joined = joinProjectPath(root, rel);
    if (!('error' in joined)) bus.emit('open-file', { path: joined.abs });
  }
  return result;
}

/** Plan mode: write the FULL plan markdown to a named file in .blazeide/plans,
 *  open it in the editor, and show a SHORT summary card in chat with an
 *  "Implement plan" action. */
async function execWritePlan(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  const card = createToolCard(call, { gated: false });
  deps.view.mount(card.element);
  card.setRunning();

  const root = state.projectRoot;
  if (!root) { card.setResult('Error: no project folder is open.', true); return 'Error: no project folder is open.'; }

  const args = parseArgs(call.arguments);
  const md = args && typeof args.markdown === 'string' ? args.markdown : '';
  const title = args && typeof args.title === 'string' ? args.title.trim() : '';
  const summary = args && typeof args.summary === 'string' ? args.summary.trim() : '';
  if (!md.trim()) { card.setResult('Error: empty plan.', true); return 'Error: the plan was empty.'; }

  // Name the file after the plan title (slugged), so plans are recognizable.
  const slug = (title || 'plan')
    .toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48) || 'plan';
  const rel = `.blazeide/plans/${slug}.md`;
  const joined = joinProjectPath(root, rel);
  if ('error' in joined) { card.setResult(`Error: ${joined.error}`, true); return `Error: ${joined.error}`; }

  // Prepend a title heading if the model didn't already.
  const fullMd = title && !md.trimStart().startsWith('#') ? `# ${title}\n\n${md}` : md;
  try {
    await window.void.fsWriteFile(joined.abs, fullMd);
  } catch (err) {
    const msg = `Error: could not save plan (${err instanceof Error ? err.message : 'write failed'}).`;
    card.setResult(msg, true);
    return msg;
  }

  card.setResult(`Saved plan to ${rel}.`, false);
  // Open the full plan in the editor so the user reads it there.
  bus.emit('open-file', { path: joined.abs });
  // Chat gets only the SHORT summary + an Implement action.
  deps.view.mountPlan(title || 'Plan', summary || 'Plan ready — open the file for the full details.', () => bus.emit('build-plan', { path: rel }));

  const build = approvalLevel() === 'full';
  if (build) {
    bus.emit('build-plan', { path: rel });
  }
  return `Plan saved to ${rel} (opened in the editor) and summarized for the user. ${build ? 'Auto-building now.' : 'Waiting for the user to implement it.'}`;
}

/** Write a file to disk, update it in the editor if open, and open it so the
 *  user sees the result. The sidebar refreshes via the fs watcher. */
async function applyEdit(editor: EditorArea, abs: string, content: string): Promise<void> {
  await window.void.fsWriteFile(abs, content);
  editor.syncOpenFile(abs, content);
  bus.emit('open-file', { path: abs });
}

/** generate_video: animate a project image into an .mp4 with a BlazeAPI video model. */
async function execGenerateVideo(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  const card = createToolCard(call, { gated: false });
  deps.view.mount(card.element);
  card.setRunning();
  const root = state.projectRoot;
  if (!root) { card.setResult('Error: no project folder is open.', true); return 'Error: no project folder is open.'; }
  const args = parseArgs(call.arguments);
  const image = args && typeof args.image === 'string' ? args.image : '';
  const prompt = args && typeof args.prompt === 'string' ? args.prompt : '';
  const rel = args && typeof args.path === 'string' ? args.path : '';
  if (!rel || (!image && !prompt)) {
    const msg = 'Error: generate_video needs path (where to save the .mp4) and a prompt (plus image to animate a still).';
    card.setResult(msg, true);
    return msg;
  }
  const result = await window.void.aiGenerateMedia({ kind: 'video', prompt, path: rel, image: image || undefined, projectRoot: root });
  const isErr = result.startsWith('Error:');
  card.setResult(result, isErr);
  if (!isErr) {
    const joined = joinProjectPath(root, rel);
    if (!('error' in joined)) bus.emit('open-file', { path: joined.abs });
  }
  return result;
}

/** The card of a call, with Approve / Reject when the level asks for it.
 *  Resolves null (and marks the card) when the user rejects. */
async function approvedCard(call: ToolCall, deps: ToolExecDeps): Promise<ToolCard | null> {
  const { ask, why } = await mustAsk(call);
  const card = createToolCard(call, { gated: ask, why });
  deps.view.mount(card.element);
  if (!(await card.approval)) {
    card.setRejected();
    return null;
  }
  card.setRunning();
  return card;
}

async function execWeb(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  const card = await approvedCard(call, deps);
  if (!card) return 'User rejected this action.';
  const r = await window.void.webTool(call.name, call.arguments).catch((e: unknown) => ({ text: `Error: ${String(e)}`, images: [] as string[] }));
  card.setResult(r.text, r.text.startsWith('Error:'));
  for (const url of r.images) {
    const img = document.createElement('img');
    img.className = 'agent-shot';
    img.src = url;
    img.alt = 'Web page screenshot';
    deps.view.mount(img);
    deps.shots?.push({ label: 'a web page', url });
  }
  return r.text;
}

/** Execute one tool call and return the tool-result string for the conversation. */
export async function execToolCall(call: ToolCall, deps: ToolExecDeps): Promise<string> {
  // ask_question: pause and let the user answer in the input area.
  if (call.name === 'ask_question') {
    return execAskQuestion(call, deps);
  }

  // Image generation: produced by the backend and written to the project by
  // the main process.
  if (call.name === 'generate_image' || call.name === 'generate_video') {
    // These cost money: at the Ask level they wait for an OK.
    if ((await mustAsk(call)).ask) {
      const gate = createToolCard(call, { gated: true });
      deps.view.mount(gate.element);
      const ok = await gate.approval;
      if (!ok) { gate.setRejected(); return 'User rejected this action.'; }
      gate.element.remove();
    }
    return call.name === 'generate_image' ? execGenerateMedia(call, deps) : execGenerateVideo(call, deps);
  }

  if (call.name.startsWith('web_')) {
    return execWeb(call, deps);
  }

  // Reading one of the user's own skills never asks.
  if (call.name === 'use_skill') {
    const card = createToolCard(call, { gated: false });
    deps.view.mount(card.element);
    card.setRunning();
    const text = await window.void.skillsUse(call.arguments).catch((e: unknown) => `Error: ${String(e)}`);
    card.setResult(text, text.startsWith('Error:'));
    return text;
  }

  // The agent's own browser (the Preview tab).
  if (call.name === 'browser') {
    const card = createToolCard(call, { gated: false });
    deps.view.mount(card.element);
    card.setRunning();
    const args = parseArgs(call.arguments) ?? {};
    const r = await agentBrowser(args as Parameters<typeof agentBrowser>[0]);
    card.setResult(r.text, !!r.isError);
    if (r.image) {
      const img = document.createElement('img');
      img.className = 'agent-shot';
      img.src = r.image;
      img.alt = 'Browser screenshot';
      deps.view.mount(img);
      deps.shots?.push({ label: r.text.split(':')[0], url: r.image });
    }
    return r.text;
  }

  // Plan mode: save the plan to .blazeide/plan.md and offer to build it.
  if (call.name === 'write_plan') {
    return execWritePlan(call, deps);
  }

  if (!MUTATING.has(call.name)) {
    const card = await approvedCard(call, deps);
    if (!card) return 'User rejected this action.';
    const result = await window.void.aiRunTool(call.name, call.arguments, state.projectRoot ?? '');
    card.setResult(result, result.startsWith('Error:'));
    return result;
  }

  // Mutating tool: the diff view is the approval surface, so no buttons here.
  const card = createToolCard(call, { gated: false });
  deps.view.mount(card.element);
  card.setRunning();

  const fail = (msg: string): string => {
    card.setResult(msg, true);
    return msg;
  };

  const root = state.projectRoot;
  if (!root) return fail('Error: no project folder is open.');

  const args = parseArgs(call.arguments);
  if (!args) return fail('Error: tool arguments were not valid JSON.');

  const rel = typeof args.path === 'string' ? args.path : '';
  if (!rel) return fail('Error: missing required argument: path');
  const joined = joinProjectPath(root, rel);
  if ('error' in joined) return fail(`Error: ${joined.error}`);
  const abs = joined.abs;

  // Quiet read: a brand-new file legitimately doesn't exist yet, so use the
  // non-throwing variant (avoids a noisy main-process error log). originalRaw
  // is null when the file didn't exist — needed so undo can delete new files.
  const originalRaw = await window.void.fsTryReadFile(abs);
  const original = originalRaw ?? '';

  let modified: string;
  if (call.name === 'apply_diff') {
    const diff = typeof args.diff === 'string' ? args.diff : '';
    const res = applyDiff(original, diff);
    if (!res.ok) return fail(`Error: ${res.error}`);
    modified = res.content;
  } else {
    modified = typeof args.content === 'string' ? args.content : '';
  }

  if (modified === original) {
    const msg = `No change needed for ${rel} (content identical).`;
    card.setResult(msg, false);
    return msg;
  }

  // How the write gets applied:
  //  - Edits, Auto and YOLO: apply directly and keep moving (every change can
  //    be reverted from the chat).
  //  - Ask: show the diff and wait for Accept/Reject.
  let accepted: boolean;
  if (approvalLevel() !== 'ask') {
    await applyEdit(deps.editor, abs, modified);
    accepted = true;
  } else {
    accepted = await deps.editor.showDiff(abs, original, modified);
  }

  // Record for the run's changes summary + undo (only if it landed on disk).
  if (accepted) deps.changes.record(abs, rel, originalRaw, modified);

  const result = accepted ? `Applied edit to ${rel}.` : `User rejected the edit to ${rel}.`;
  card.setResult(result, false);
  return result;
}

/** Flatten the project tree into a capped path list for model context. */
function flattenTree(node: FileNode, root: string, out: string[]): void {
  if (out.length > 4000) return;
  for (const child of node.children ?? []) {
    if (SKIP_DIRS.has(child.name)) continue;
    const rel = child.path.startsWith(root) ? child.path.slice(root.length).replace(/^\/+/, '') : child.path;
    out.push(child.isDirectory ? `${rel}/` : rel);
    if (child.isDirectory) flattenTree(child, root, out);
  }
}

/** Build the <environment_details> block listing project files, so the model
 *  can locate and edit files without one being open. Empty when no project. */
export async function buildWorkspaceContext(): Promise<string> {
  const root = state.projectRoot;
  if (!root) return '';
  let tree: FileNode;
  try {
    tree = await window.void.fsReadTree(root);
  } catch {
    return '';
  }
  const paths: string[] = [];
  flattenTree(tree, root, paths);

  let listing = '';
  let truncated = false;
  for (const p of paths) {
    if (listing.length + p.length + 1 > TREE_CONTEXT_CAP) {
      truncated = true;
      break;
    }
    listing += `${p}\n`;
  }
  if (truncated) listing += '...[file list truncated]\n';

  return [
    '<environment_details>',
    `Workspace root: ${state.projectName || root}`,
    'Project files:',
    listing.trimEnd(),
    '</environment_details>',
  ].join('\n');
}
