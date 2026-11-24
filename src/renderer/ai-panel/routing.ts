// BlazeAPI routing choice for chat requests. Normal and Direct list the same
// models under slightly different ids, so switching reloads the list and moves
// the selected model to its twin on the other side.

import type { AIRouting } from '../../shared/types';
import { bus, state } from '../state';
import { normModelId } from './chatStore';

export const ROUTING_CHOICES: Array<{ value: AIRouting; label: string; hint: string; admin?: boolean }> = [
  { value: '', label: 'Account', hint: 'Use the routing set on your BlazeAPI account' },
  { value: 'normal', label: 'Normal', hint: 'Normal routing: the most models and the lowest prices' },
  { value: 'direct', label: 'Direct', hint: 'Direct (trusted) routing: model makers and trusted resellers only' },
  { value: 'cheapest', label: 'Cheapest', hint: 'Owner only: Normal routing, cheapest working supplier first', admin: true },
];

export function routingChoices(): typeof ROUTING_CHOICES {
  return ROUTING_CHOICES.filter((c) => !c.admin || state.session?.admin);
}

/** Switch routing, reload the model list for it, keep the same model selected. */
export async function setRouting(r: AIRouting): Promise<void> {
  state.settings.aiRouting = r;
  await window.void.settingsSet({ aiRouting: r });
  const models = await window.void.aiRefreshModels();
  state.models = models;
  const sel = state.settings.selectedModel;
  if (!models.some((m) => m.id === sel)) {
    const twin = models.find((m) => m.kind === 'chat' && normModelId(m.id) === normModelId(sel));
    if (twin) {
      state.settings.selectedModel = twin.id;
      await window.void.settingsSet({ selectedModel: twin.id });
    }
  }
  bus.emit('models-changed', models);
}
