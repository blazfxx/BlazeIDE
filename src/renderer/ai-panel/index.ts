// AI panel assembly: header (provider + model selector), chat, input.

import { el } from '../state';
import type { EditorArea } from '../editor/index';
import { initModelSelector } from './modelSelector';
import { createChatView } from './chat';
import { createController } from './controller';
import { initChatControls } from './chatControls';
import { initInput } from './input';

export function initAIPanel(host: HTMLElement, editor: EditorArea): void {
  const header = el('div', 'ai-header');
  initModelSelector(header);
  host.appendChild(header);

  // Controls row (context meter + new/history/compact) sits under the header.
  const controlsHost = el('div', 'ai-controls-host');
  host.appendChild(controlsHost);

  const chatHost = el('div', 'ai-chat-host');
  host.appendChild(chatHost);
  const view = createChatView(chatHost);

  const controller = createController(view, editor);
  initChatControls(controlsHost, controller);
  initInput(host, controller);
}
