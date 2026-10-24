// Type shim for the lazily-imported Monaco bundle, which is built as a
// separate esbuild entry (dist/renderer/monaco.js) and marked external in
// the main renderer bundle.
declare module '*monaco.js' {
  import type * as monacoNS from 'monaco-editor';
  export const monaco: typeof monacoNS;
}
