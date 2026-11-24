// Regenerates apply_diff_cases.json from the TypeScript engine
// (src/shared/applyDiff.ts). The Rust port (crates/blaze-core/src/edit.rs)
// must produce the same output for every case.
//   node tests/fixtures/make_apply_diff_cases.mjs
import { build } from 'esbuild';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const out = await build({ entryPoints: [here + '../../src/shared/applyDiff.ts'], bundle: true, format: 'esm', write: false, platform: 'neutral' });
const mod = await import('data:text/javascript;base64,' + Buffer.from(out.outputFiles[0].text).toString('base64'));

const B = (search, replace, hint = '') => `<<<<<<< SEARCH\n${hint}${search}\n=======\n${replace}\n>>>>>>> REPLACE`;
const cases = [
  ['simple', 'a\nb\nc\n', B('b', 'B')],
  ['two blocks in order', 'one\ntwo\nthree\nfour\n', B('two', '2') + '\n' + B('four', '4')],
  ['blocks out of order fail', 'one\ntwo\nthree\n', B('three', '3') + '\n' + B('one', '1')],
  ['indent drift matches trimmed', 'fn x() {\n    let a = 1;\n    let b = 2;\n}\n', B('let a = 1;\nlet b = 2;', '    let a = 10;\n    let b = 20;')],
  ['ambiguous trimmed refuses', 'x\n  y\nx\n  y\n', B('  x\ny', 'z')],
  ['no match', 'hello\n', B('bye', 'x')],
  ['empty search replaces file', 'old\n', B('', 'brand new\nfile')],
  ['create from empty', '', B('', 'first')],
  ['crlf preserved', 'a\r\nb\r\nc\r\n', B('b', 'B\nB2')],
  ['start line hint picks second copy', 'k\nv\nk\nv\n', B('k', 'K', ':start_line:3\n')],
  ['bad hint falls back', 'k\nv\n', B('k', 'K', ':start_line:99\n')],
  ['fence line allowed', 'a\nb\n', '<<<<<<< SEARCH\n-------\na\n=======\nA\n>>>>>>> REPLACE'],
  ['long markers', 'a\nb\n', '<<<<<<<<<< SEARCH\na\n==========\nA\n>>>>>>>>>> REPLACE'],
  ['missing separator', 'a\n', '<<<<<<< SEARCH\na\n>>>>>>> REPLACE'],
  ['missing close', 'a\n', '<<<<<<< SEARCH\na\n=======\nA'],
  ['no blocks', 'a\n', 'just text'],
  ['unicode content', 'héllo wörld\n日本語\nend\n', B('日本語', 'にほんご')],
  ['delete lines', 'keep\ndrop1\ndrop2\nkeep2\n', B('drop1\ndrop2\n', '')],
  ['search marker with angle', 'a\n', '<<<<<<< SEARCH>\na\n=======\nb\n>>>>>>> REPLACE'],
  ['text around blocks ignored', 'a\nb\n', 'Here is the change:\n' + B('a', 'x') + '\nDone.'],
];
const rows = cases.map(([name, original, diff]) => {
  const r = mod.applyDiff(original, diff);
  return r.ok ? { name, original, diff, ok: true, content: r.content } : { name, original, diff, ok: false, error: r.error };
});
writeFileSync(here + 'apply_diff_cases.json', JSON.stringify(rows, null, 1) + '\n');
console.log(`${rows.length} cases`);
