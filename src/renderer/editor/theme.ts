// The single dark theme + option mapping. Token colors stay within the app
// palette — no purple.

import type * as MonacoTypes from 'monaco-editor';
import type { AppSettings } from '../../shared/types';
import type { Monaco } from '../monaco-loader-client';

export function editorOptions(s: AppSettings): MonacoTypes.editor.IEditorOptions {
  return {
    fontSize: s.editorFontSize,
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', 'Menlo', 'Consolas', monospace",
    minimap: { enabled: s.editorMinimap },
    lineNumbers: s.editorRelativeLineNumbers ? 'relative' : 'on',
    wordWrap: s.editorWordWrap ? 'on' : 'off',
    automaticLayout: true,
    scrollBeyondLastLine: false,
    renderLineHighlight: 'line',
    padding: { top: 8 },
    // Options accepted at runtime but absent from this d.ts's IEditorOptions:
    //  - wordBasedSuggestions 'off': skip scanning open docs for a word index.
    //  - semanticHighlighting off: Monarch (main-thread) coloring is enough and
    //    avoids booting the language worker on first open.
    ...({ wordBasedSuggestions: 'off', 'semanticHighlighting.enabled': false } as Record<string, unknown>),
  };
}

export function defineVoidTheme(monaco: Monaco): void {
  monaco.editor.defineTheme('void-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: 'ffffff', fontStyle: 'bold' },
      { token: 'string', foreground: 'b0b0b0' },
      { token: 'comment', foreground: '6a6a6a', fontStyle: 'italic' },
      { token: 'number', foreground: 'd0d0d0' },
      { token: 'type', foreground: 'e0e0e0' },
      { token: 'function', foreground: 'f5f5f5' },
    ],
    colors: {
      'editor.background': '#000000',
      'editor.foreground': '#f5f5f5',
      'editor.lineHighlightBackground': '#0a0a0a',
      'editorLineNumber.foreground': '#4a4a4a',
      'editorLineNumber.activeForeground': '#8a8a8a',
      'editorCursor.foreground': '#ffffff',
      'editor.selectionBackground': '#262626',
      'editorWidget.background': '#141414',
      'editorWidget.border': '#2e2e2e',
    },
  });
  monaco.editor.defineTheme('void-light', {
    base: 'vs',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: '111111', fontStyle: 'bold' },
      { token: 'string', foreground: '4a4a4a' },
      { token: 'comment', foreground: '9a9a9a', fontStyle: 'italic' },
      { token: 'number', foreground: '333333' },
      { token: 'type', foreground: '222222' },
      { token: 'function', foreground: '000000' },
    ],
    colors: {
      'editor.background': '#ffffff',
      'editor.foreground': '#111111',
      'editor.lineHighlightBackground': '#f3f3f3',
      'editorLineNumber.foreground': '#b0b0b0',
      'editorLineNumber.activeForeground': '#6a6a6a',
      'editorCursor.foreground': '#111111',
      'editor.selectionBackground': '#d8d8d8',
      'editorWidget.background': '#eeeeee',
      'editorWidget.border': '#d0d0d0',
    },
  });
}

/** Monaco theme name for the app theme. */
export function monacoThemeFor(theme: AppSettings['theme']): string {
  return theme === 'light' ? 'void-light' : 'void-dark';
}
