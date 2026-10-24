// Ghost-text autocomplete: Monaco asks for an inline completion after a short
// pause in typing; Rust asks a model (see ai_complete) and answers with text to
// insert at the cursor. Tab accepts. Quiet on any failure.

import type { Monaco } from '../monaco-loader-client';
import { state } from '../state';

const PAUSE_MS = 350;
const MIN_PREFIX = 2;

function sleep(ms: number): Promise<void> {
  return new Promise((r) => window.setTimeout(r, ms));
}

export function registerAutocomplete(m: Monaco): void {
  let lastKey = '';
  let lastText = '';

  m.languages.registerInlineCompletionsProvider({ pattern: '**' }, {
    async provideInlineCompletions(model, position, _ctx, token) {
      const none = { items: [] };
      if (!state.settings.aiAutocomplete || !state.session) return none;
      await sleep(PAUSE_MS);
      if (token.isCancellationRequested) return none;

      const last = model.getLineCount();
      const prefix = model.getValueInRange({ startLineNumber: 1, startColumn: 1, endLineNumber: position.lineNumber, endColumn: position.column });
      const suffix = model.getValueInRange({ startLineNumber: position.lineNumber, startColumn: position.column, endLineNumber: last, endColumn: model.getLineMaxColumn(last) });
      if (prefix.trim().length < MIN_PREFIX) return none;

      const key = `${model.uri.path}\u0000${prefix.slice(-600)}\u0000${suffix.slice(0, 200)}`;
      let text = key === lastKey ? lastText : '';
      if (!text) {
        text = await window.void.aiComplete({ prefix, suffix, filename: model.uri.path }).catch(() => '');
        if (token.isCancellationRequested) return none;
        lastKey = key;
        lastText = text;
      }
      if (!text) return none;
      return {
        items: [{ insertText: text, range: new m.Range(position.lineNumber, position.column, position.lineNumber, position.column) }],
      };
    },
    freeInlineCompletions() {},
  });
}
