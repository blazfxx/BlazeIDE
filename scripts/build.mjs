// Builds the UI into dist/ (what Tauri bundles): the app bundle, the lazily
// loaded Monaco bundle and its workers, HTML, CSS and static assets. Fails if
// anything that looks like a credential ended up in the output.

import * as esbuild from 'esbuild';
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, copyFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = resolve(root, 'dist');
const release = process.env.BLAZE_RELEASE === '1' || process.env.TAURI_ENV_DEBUG === 'false';
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

const common = {
  bundle: true,
  // No source maps in release: they would ship the full source in the app.
  sourcemap: !release,
  minify: release,
  logLevel: 'info',
  absWorkingDir: root,
  legalComments: 'none',
  target: ['safari15', 'chrome105'],
};


// Tauri's `freezePrototype` (kept on) makes `obj.toString = fn` throw for any
// plain object, and Monaco's compiled TypeScript namespaces do exactly that
// (KeyCodeUtils.toString = ...). Rewrite those assignments into
// defineProperty, which is allowed on an object whose prototype is frozen.
const frozenProtoSafe = {
  name: 'frozen-proto-safe',
  setup(b) {
    b.onLoad({ filter: /node_modules[\\/]monaco-editor[\\/].*\.js$/ }, (args) => {
      const src = readFileSync(args.path, 'utf8');
      const out = src.replace(
        /^(\s*)([A-Za-z_$][\w$]*)\.(toString|valueOf|constructor) = ([A-Za-z_$][\w$]*);/gm,
        (_m, ws, obj, prop, val) => `${ws}Object.defineProperty(${obj}, '${prop}', { value: ${val}, writable: true, configurable: true, enumerable: true });`,
      );
      // Method-name keyed proxy tables include 'constructor'; a null-prototype
      // object can hold it.
      const out2 = out.replace(/const foreignProxy = \{\};/g, 'const foreignProxy = Object.create(null);');
      // Monarch keyword tables (JS/TS keywords include 'constructor'): without
      // this every .js/.ts file failed to tokenize under a frozen prototype.
      const out3 = out2.replace(
        /function arrayToHash\(array\) \{\s*const result = \{\};\s*for \(const e of array\) \{\s*result\[e\] = true;\s*\}/,
        'function arrayToHash(array) { const result = {}; for (const e of array) { Object.defineProperty(result, e, { value: true, writable: true, enumerable: true, configurable: true }); }',
      );
      if (args.path.endsWith('monarchCompile.js') && out3 === out2) throw new Error('frozen-proto-safe: arrayToHash patch no longer applies');
      return { contents: out3, loader: 'js' };
    });
  },
};

// App: ESM with code splitting. Monaco stays out of the startup bundle; the
// loader dynamic-imports './monaco.js', built below.
await esbuild.build({
  ...common,
  entryPoints: ['src/renderer/index.ts'],
  platform: 'browser',
  format: 'esm',
  splitting: true,
  outdir: 'dist',
  external: ['./monaco.js'],
  loader: { '.ttf': 'file', '.woff': 'file', '.woff2': 'file' },
});

await esbuild.build({
  ...common,
  entryPoints: { monaco: 'src/renderer/monaco-loader.ts' },
  plugins: [frozenProtoSafe],
  platform: 'browser',
  format: 'esm',
  outdir: 'dist',
  loader: { '.ttf': 'file', '.woff': 'file', '.woff2': 'file' },
});

await esbuild.build({
  ...common,
  entryPoints: {
    'editor.worker': 'monaco-editor/esm/vs/editor/editor.worker.js',
    'json.worker': 'monaco-editor/esm/vs/language/json/json.worker.js',
    'css.worker': 'monaco-editor/esm/vs/language/css/css.worker.js',
    'html.worker': 'monaco-editor/esm/vs/language/html/html.worker.js',
    'ts.worker': 'monaco-editor/esm/vs/language/typescript/ts.worker.js',
  },
  platform: 'browser',
  format: 'iife',
  outdir: 'dist/workers',
});

copyFileSync(resolve(root, 'src/renderer/index.html'), resolve(dist, 'index.html'));
cpSync(resolve(root, 'src/renderer/styles'), resolve(dist, 'styles'), { recursive: true });
if (existsSync(resolve(root, 'src/renderer/assets'))) cpSync(resolve(root, 'src/renderer/assets'), resolve(dist, 'assets'), { recursive: true });

// Secret guard: generic credential shapes, so nothing secret has to be written
// into this script to look for it.
const SECRET_PATTERNS = [
  /-----BEGIN [A-Z ]*PRIVATE KEY-----/,
  /\bsk-(?:blaze|ant|proj|live)[-_][A-Za-z0-9_-]{20,}/,
  /\bnvapi-[A-Za-z0-9_-]{30,}/,
  /\blrsk-[a-f0-9]{40,}/,
  /\bgh[pousr]_[A-Za-z0-9]{30,}/,
  /\bAKIA[0-9A-Z]{16}\b/,
  /\bxox[abprs]-[A-Za-z0-9-]{20,}/,
];
function scan(dir) {
  const hits = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) { hits.push(...scan(p)); continue; }
    if (!/\.(js|mjs|json|html|css|map)$/.test(e.name)) continue;
    const text = readFileSync(p, 'utf8');
    for (const re of SECRET_PATTERNS) if (re.test(text)) hits.push(`${p} matches ${re}`);
  }
  return hits;
}
const leaks = scan(dist);
if (leaks.length) {
  console.error('\nBUILD FAILED: possible secrets in dist/:');
  for (const l of leaks) console.error('  ' + l);
  process.exit(1);
}
console.log(`build complete (${release ? 'release' : 'debug'})`);
