// Agent tool-call cards: collapsible blocks with approval flow and status edge.

import type { ToolCall } from '../../shared/types';
import { el, escapeHtml, state } from '../state';
import { iconAgent, iconCheck, iconChevronDown, iconChevronRight, iconError } from '../icons/index';

const RESULT_DISPLAY_CAP = 4000;

export interface ToolCard {
  element: HTMLElement;
  /** Resolves true if approved (or auto-approved), false if rejected. */
  approval: Promise<boolean>;
  setRunning(): void;
  setResult(result: string, isError: boolean): void;
  setRejected(): void;
}

function argSummary(call: ToolCall): string {
  try {
    const parsed: unknown = JSON.parse(call.arguments || '{}');
    if (parsed !== null && typeof parsed === 'object') {
      const o = parsed as Record<string, unknown>;
      const v = o.path ?? o.command ?? o.query ?? '';
      if (typeof v === 'string' && v) return v.length > 60 ? `${v.slice(0, 60)}...` : v;
    }
  } catch {
    // fall through
  }
  return '';
}

function prettyArgs(call: ToolCall): string {
  try {
    const parsed: unknown = JSON.parse(call.arguments || '{}');
    if (parsed === null || typeof parsed !== 'object') return call.arguments;
    const o = { ...(parsed as Record<string, unknown>) };
    // Don't dump big file bodies into the card — show a short note instead.
    if (typeof o.content === 'string') {
      const lines = o.content.split('\n').length;
      o.content = `[${o.content.length} chars, ${lines} line${lines === 1 ? '' : 's'}]`;
    }
    if (typeof o.diff === 'string' && o.diff.length > 200) {
      o.diff = `${o.diff.slice(0, 200)}…`;
    }
    if (typeof o.markdown === 'string') {
      const lines = o.markdown.split('\n').length;
      o.markdown = `[plan: ${o.markdown.length} chars, ${lines} line${lines === 1 ? '' : 's'}]`;
    }
    return JSON.stringify(o, null, 2);
  } catch {
    return call.arguments;
  }
}

export interface ToolCardOptions {
  /** When false, render no Approve/Reject buttons and resolve approval to true
   *  immediately. The approval surface lives elsewhere (e.g. the diff view). */
  gated?: boolean;
}

export function createToolCard(call: ToolCall, opts: ToolCardOptions = {}): ToolCard {
  const block = el('div', 'tool-block pending');
  const head = el('div', 'tool-head');
  const chevron = el('span', 'icon icon-12 tool-chevron', iconChevronRight);
  const status = el('span', 'icon icon-12 tool-status', iconAgent);
  const name = el('span', 'tool-name', escapeHtml(call.name));
  const summary = el('span', 'tool-summary', escapeHtml(argSummary(call)));
  head.append(chevron, status, name, summary);
  block.appendChild(head);

  const body = el('div', 'tool-body');
  body.style.display = 'none';
  const argsEl = el('pre', 'tool-args', escapeHtml(prettyArgs(call)));
  body.appendChild(argsEl);
  const resultEl = el('pre', 'tool-result');
  resultEl.style.display = 'none';
  body.appendChild(resultEl);
  block.appendChild(body);

  let expanded = false;
  head.addEventListener('click', () => {
    expanded = !expanded;
    body.style.display = expanded ? 'block' : 'none';
    chevron.innerHTML = expanded ? iconChevronDown : iconChevronRight;
  });

  let settle: (approved: boolean) => void = () => undefined;
  const approval = new Promise<boolean>((resolve) => {
    settle = resolve;
  });

  const gated = opts.gated !== false;
  if (!gated || state.settings.aiAutoApprove) {
    settle(true);
  } else {
    const actions = el('div', 'tool-actions');
    const approveBtn = el('button', 'primary', 'Approve');
    const rejectBtn = el('button', '', 'Reject');
    actions.append(approveBtn, rejectBtn);
    block.appendChild(actions);
    approveBtn.addEventListener('click', () => {
      actions.remove();
      settle(true);
    });
    rejectBtn.addEventListener('click', () => {
      actions.remove();
      settle(false);
    });
  }

  function setState(cls: 'pending' | 'running' | 'done' | 'error'): void {
    block.classList.remove('pending', 'running', 'done', 'error');
    block.classList.add(cls);
  }

  return {
    element: block,
    approval,
    setRunning() {
      setState('running');
      status.innerHTML = '<span class="spinner"></span>';
    },
    setResult(result, isError) {
      setState(isError ? 'error' : 'done');
      status.innerHTML = isError ? iconError : iconCheck;
      const display = result.length > RESULT_DISPLAY_CAP ? `${result.slice(0, RESULT_DISPLAY_CAP)}\n...[truncated]` : result;
      resultEl.textContent = display;
      resultEl.style.display = 'block';
    },
    setRejected() {
      setState('error');
      status.innerHTML = iconError;
      resultEl.textContent = 'User rejected this action.';
      resultEl.style.display = 'block';
    },
  };
}
