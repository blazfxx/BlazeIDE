// Built as its own esbuild entry (dist/renderer/monaco.js). Pulls in the full
// Monaco editor; loaded lazily by monaco-loader-client when the first file opens.
// Monaco's ESM modules import their own CSS — esbuild emits it as monaco.css,
// which monaco-loader-client links before resolving.
import * as monaco from 'monaco-editor';

interface EnvHost {
  MonacoEnvironment?: { getWorkerUrl(moduleId: string, label: string): string };
}

(globalThis as EnvHost).MonacoEnvironment = {
  getWorkerUrl(_moduleId: string, label: string): string {
    if (label === 'json') return './workers/json.worker.js';
    if (label === 'css' || label === 'scss' || label === 'less') return './workers/css.worker.js';
    if (label === 'html' || label === 'handlebars' || label === 'razor') return './workers/html.worker.js';
    if (label === 'typescript' || label === 'javascript') return './workers/ts.worker.js';
    return './workers/editor.worker.js';
  },
};

export { monaco };
