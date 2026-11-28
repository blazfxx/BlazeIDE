// How much the chat agent may do without asking: Ask, Edits, Auto (all but
// destructive actions) or YOLO. Kept in the aiApproval setting.

import type { ApprovalLevel, ToolCall } from '../../shared/types';
import { bus, state } from '../state';
import { confirmModal } from '../sidebar/confirm';

export const APPROVALS: Array<{ value: ApprovalLevel; label: string; hint: string }> = [
  { value: 'ask', label: 'Ask', hint: 'Ask before every edit and command' },
  { value: 'edits', label: 'Edits', hint: 'Edit files, ask before commands' },
  { value: 'auto', label: 'Auto', hint: 'Run everything except destructive actions' },
  { value: 'full', label: 'YOLO', hint: 'Approve everything, including destructive commands' },
];

export function approvalLevel(): ApprovalLevel {
  // Before this setting there was only an "auto-approve everything" switch.
  return state.settings.aiApproval || (state.settings.aiAutoApprove ? 'full' : 'edits');
}

/** Change the level; YOLO is confirmed first. Returns the level now in use. */
export async function setApproval(v: ApprovalLevel): Promise<ApprovalLevel> {
  if (v === 'full' && approvalLevel() !== 'full') {
    const r = await confirmModal({
      title: 'Turn on YOLO mode?',
      message: 'The agent will run every command without asking, including ones that delete files or throw away work. Web pages and files it reads can try to trick it. Use it only where you can lose everything, such as a project saved in git elsewhere.',
      confirmLabel: 'Turn on YOLO',
      danger: true,
    });
    if (!r.confirmed) return approvalLevel();
  }
  const patch = { aiApproval: v, aiAutoApprove: v === 'full' };
  Object.assign(state.settings, patch);
  await window.void.settingsSet(patch);
  bus.emit('ai-approval-changed', undefined);
  return v;
}

/** Whether this call needs the user's OK, and (in Auto) why. */
export async function mustAsk(call: ToolCall): Promise<{ ask: boolean; why?: string }> {
  const level = approvalLevel();
  if (level === 'full') return { ask: false };
  const web = call.name.startsWith('web_');
  if (level === 'auto') {
    if (call.name !== 'run_command' && call.name !== 'web_interact') return { ask: false };
    // The core decides, the same way as for bots and the CLI. If it cannot
    // be asked, ask the user.
    const why = await window.void.approvalRisk(call.name, call.arguments).catch(() => 'it could not be checked');
    return why ? { ask: true, why } : { ask: false };
  }
  if (call.name === 'web_search') return { ask: false };
  if (call.name === 'web_interact' || call.name === 'run_command') return { ask: true };
  if (web || call.name === 'generate_image' || call.name === 'generate_video') return { ask: level === 'ask' };
  return { ask: false };
}
