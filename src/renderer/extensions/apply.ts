// Turns installed VS Code extensions into Monaco features: colour themes and
// snippets. Nothing here runs extension code; it only reads their JSON.

import type * as MonacoTypes from 'monaco-editor';
import type { AppSettings, ExtContributions } from '../../shared/types';
import type { Monaco } from '../monaco-loader-client';
import { monacoThemeFor } from '../editor/theme';

type Base = 'vs' | 'vs-dark' | 'hc-black';

/** TextMate scope prefix -> the nearest token name Monaco's tokenizers emit. */
const SCOPES: Array<[string, string]> = [
  ['comment', 'comment'],
  ['string.regexp', 'regexp'],
  ['string', 'string'],
  ['constant.numeric', 'number'],
  ['constant.language', 'keyword'],
  ['constant', 'constant'],
  ['keyword.operator', 'operator'],
  ['keyword', 'keyword'],
  ['storage', 'keyword'],
  ['entity.name.type', 'type'],
  ['entity.name.class', 'type'],
  ['support.class', 'type'],
  ['support.type', 'type'],
  ['entity.name.tag', 'tag'],
  ['entity.other.attribute-name', 'attribute.name'],
  ['variable', 'variable'],
  ['punctuation', 'delimiter'],
  ['meta.decorator', 'annotation'],
];

const COLOR = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

export function themeKey(extId: string, label: string): string {
  return `ext:${extId}/${label}`;
}

type TokenColor = { scope?: string | string[]; settings?: { foreground?: string; background?: string; fontStyle?: string } };

/** VS Code theme JSON -> Monaco theme data. Unknown scopes are skipped. */
export function convertTheme(base: string, json: Record<string, unknown>): MonacoTypes.editor.IStandaloneThemeData {
  const colors: Record<string, string> = {};
  for (const [k, v] of Object.entries((json.colors as Record<string, unknown>) ?? {})) {
    if (typeof v === 'string' && COLOR.test(v)) colors[k] = v;
  }
  const rules = new Map<string, MonacoTypes.editor.ITokenThemeRule>();
  for (const tc of (Array.isArray(json.tokenColors) ? json.tokenColors : []) as TokenColor[]) {
    const st = tc?.settings;
    if (!st) continue;
    if (!tc.scope) {
      // The theme-wide defaults.
      if (st.foreground && COLOR.test(st.foreground) && !colors['editor.foreground']) colors['editor.foreground'] = st.foreground;
      if (st.background && COLOR.test(st.background) && !colors['editor.background']) colors['editor.background'] = st.background;
      continue;
    }
    const scopes = (Array.isArray(tc.scope) ? tc.scope : tc.scope.split(',')).map((x) => x.trim());
    for (const scope of scopes) {
      const hit = SCOPES.find(([prefix]) => scope === prefix || scope.startsWith(prefix + '.'));
      if (!hit) continue;
      const rule: MonacoTypes.editor.ITokenThemeRule = { token: hit[1] };
      if (st.foreground && COLOR.test(st.foreground)) rule.foreground = st.foreground.replace('#', '').slice(0, 6);
      if (st.fontStyle !== undefined) rule.fontStyle = st.fontStyle;
      if (rule.foreground || rule.fontStyle !== undefined) rules.set(hit[1], rule);
    }
  }
  const b: Base = base === 'vs' || base === 'hc-black' ? base : 'vs-dark';
  return { base: b, inherit: true, rules: [...rules.values()], colors };
}

interface SnippetDef { prefix?: string | string[]; body?: string | string[]; description?: string }

function snippetItems(monaco: Monaco, json: Record<string, unknown>, range: MonacoTypes.IRange): MonacoTypes.languages.CompletionItem[] {
  const out: MonacoTypes.languages.CompletionItem[] = [];
  for (const [name, def] of Object.entries(json) as Array<[string, SnippetDef]>) {
    if (!def || def.body === undefined || def.prefix === undefined) continue;
    const body = Array.isArray(def.body) ? def.body.join('\n') : String(def.body);
    for (const prefix of Array.isArray(def.prefix) ? def.prefix : [def.prefix]) {
      out.push({
        label: prefix,
        kind: monaco.languages.CompletionItemKind.Snippet,
        detail: name,
        documentation: def.description ? String(def.description) : undefined,
        insertText: body,
        insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
        range,
      });
    }
  }
  return out;
}

let disposables: MonacoTypes.IDisposable[] = [];
let known = new Set<string>();

/** (Re)load every installed extension's themes and snippets into Monaco. */
export async function applyExtensions(monaco: Monaco, settings: AppSettings): Promise<void> {
  const c: ExtContributions = await window.void.extContributions().catch(() => ({ themes: [], snippets: [] }));
  for (const d of disposables) d.dispose();
  disposables = [];
  known = new Set();
  for (const t of c.themes) {
    const key = themeKey(t.extId, t.label);
    try {
      monaco.editor.defineTheme(key, convertTheme(t.base, t.json));
      known.add(key);
    } catch { /* a malformed theme is skipped, not fatal */ }
  }
  const byLang = new Map<string, Array<Record<string, unknown>>>();
  for (const s of c.snippets) byLang.set(s.language, [...(byLang.get(s.language) ?? []), s.json]);
  for (const [language, files] of byLang) {
    disposables.push(
      monaco.languages.registerCompletionItemProvider(language, {
        provideCompletionItems(model, pos) {
          const w = model.getWordUntilPosition(pos);
          const range = { startLineNumber: pos.lineNumber, endLineNumber: pos.lineNumber, startColumn: w.startColumn, endColumn: w.endColumn };
          return { suggestions: files.flatMap((f) => snippetItems(monaco, f, range)) };
        },
      }),
    );
  }
  monaco.editor.setTheme(editorThemeName(settings));
}

/** The Monaco theme for these settings: the chosen extension theme if it is still installed. */
export function editorThemeName(s: AppSettings): string {
  return s.editorTheme && known.has(s.editorTheme) ? s.editorTheme : monacoThemeFor(s.theme);
}

export function installedThemeKeys(): string[] {
  return [...known];
}
