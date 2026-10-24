// Pure SEARCH/REPLACE diff engine plus project-path helpers. No Node APIs, so
// this module is safe to import from both the main and renderer processes.
//
// Diff format (Roo-style), one or more blocks per `diff`:
//
//   <<<<<<< SEARCH
//   :start_line:42        (optional hint)
//   -------               (optional separator)
//   <text to find>
//   =======
//   <replacement text>
//   >>>>>>> REPLACE
//
// An empty SEARCH body means "create or replace the whole file" with the
// REPLACE body. Blocks apply sequentially against the running content.

export type DiffResult = { ok: true; content: string } | { ok: false; error: string };

/** Directories never walked when building workspace context or searching. */
export const SKIP_DIRS = new Set(['node_modules', '.git', 'dist', 'release', '.DS_Store']);

interface Block {
  startLine: number; // 1-based hint, 0 = none
  search: string;
  replace: string;
}

const OPEN = /^<{3,}\s*SEARCH\s*>?\s*$/;
const SEP = /^={3,}\s*$/;
const CLOSE = /^>{3,}\s*REPLACE\s*$/;
const FENCE = /^-{3,}\s*$/;
const START_LINE = /^:start_line:\s*(\d+)\s*$/;

function detectEol(text: string): '\r\n' | '\n' {
  return text.includes('\r\n') ? '\r\n' : '\n';
}

/** Parse the raw diff text into ordered blocks. Lenient about marker length and
 *  the optional `:start_line:` / `-------` lines so minor model mistakes still
 *  apply. */
function parseBlocks(diff: string): { blocks: Block[] } | { error: string } {
  const lines = diff.replace(/\r\n/g, '\n').split('\n');
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!OPEN.test(lines[i])) {
      i++;
      continue;
    }
    i++; // consume <<<<<<< SEARCH
    let startLine = 0;
    if (i < lines.length) {
      const m = START_LINE.exec(lines[i]);
      if (m) {
        startLine = Number(m[1]);
        i++;
      }
    }
    if (i < lines.length && FENCE.test(lines[i])) i++; // optional -------

    const search: string[] = [];
    while (i < lines.length && !SEP.test(lines[i]) && !CLOSE.test(lines[i])) {
      search.push(lines[i]);
      i++;
    }
    if (i >= lines.length || !SEP.test(lines[i])) {
      return { error: 'Malformed diff: missing "=======" between SEARCH and REPLACE.' };
    }
    i++; // consume =======

    const replace: string[] = [];
    while (i < lines.length && !CLOSE.test(lines[i])) {
      replace.push(lines[i]);
      i++;
    }
    if (i >= lines.length) {
      return { error: 'Malformed diff: missing ">>>>>>> REPLACE" marker.' };
    }
    i++; // consume >>>>>>> REPLACE

    blocks.push({ startLine, search: search.join('\n'), replace: replace.join('\n') });
  }
  if (blocks.length === 0) {
    return { error: 'No SEARCH/REPLACE blocks found in the diff.' };
  }
  return { blocks };
}

/** Index in `content` (at or after `from`) where `search` begins, biased toward
 *  `startLine` when several exact matches exist. -1 if none. */
function findExact(content: string, search: string, from: number, startLine: number): number {
  if (startLine > 0) {
    // Try the hinted line first: offset of the (startLine-1)th newline.
    let off = 0;
    for (let n = 1; n < startLine && off !== -1; n++) {
      const nl = content.indexOf('\n', off);
      off = nl === -1 ? -1 : nl + 1;
    }
    if (off >= 0 && content.startsWith(search, off)) return off;
  }
  return content.indexOf(search, from);
}

/** Line-trimmed window match: find a run of lines equal to `search` after
 *  trimming each line, so indentation/trailing-space drift still matches.
 *  Returns the [start, end) character span in `content`, or null. */
function findTrimmed(content: string, search: string, from: number): { start: number; end: number } | null {
  const cLines = content.split('\n');
  const sLines = search.split('\n').map((l) => l.trim());
  // Character offset of the start of each content line.
  const offsets: number[] = [];
  let acc = 0;
  for (const l of cLines) {
    offsets.push(acc);
    acc += l.length + 1; // +1 for the newline
  }
  let match = -1;
  for (let i = 0; i + sLines.length <= cLines.length; i++) {
    if (offsets[i] < from) continue;
    let ok = true;
    for (let j = 0; j < sLines.length; j++) {
      if (cLines[i + j].trim() !== sLines[j]) {
        ok = false;
        break;
      }
    }
    if (ok) {
      if (match !== -1) return null; // ambiguous — refuse to guess
      match = i;
    }
  }
  if (match === -1) return null;
  const start = offsets[match];
  const last = match + sLines.length - 1;
  const end = offsets[last] + cLines[last].length;
  return { start, end };
}

/** Apply one or more SEARCH/REPLACE blocks to `original`. */
export function applyDiff(original: string, diff: string): DiffResult {
  const parsed = parseBlocks(diff);
  if ('error' in parsed) return { ok: false, error: parsed.error };

  const eol = detectEol(original);
  let content = original.replace(/\r\n/g, '\n');
  let cursor = 0;

  for (let b = 0; b < parsed.blocks.length; b++) {
    const block = parsed.blocks[b];
    const search = block.search.replace(/\r\n/g, '\n');
    const replace = block.replace.replace(/\r\n/g, '\n');

    // Empty SEARCH: whole-file create/replace.
    if (search.trim() === '') {
      content = replace;
      cursor = content.length;
      continue;
    }

    const exact = findExact(content, search, cursor, block.startLine);
    if (exact >= 0) {
      content = content.slice(0, exact) + replace + content.slice(exact + search.length);
      cursor = exact + replace.length;
      continue;
    }

    const trimmed = findTrimmed(content, search, cursor);
    if (trimmed) {
      content = content.slice(0, trimmed.start) + replace + content.slice(trimmed.end);
      cursor = trimmed.start + replace.length;
      continue;
    }

    return {
      ok: false,
      error: `SEARCH block ${b + 1} did not match the file. Re-read the file to get its current content, then retry with an exact SEARCH.`,
    };
  }

  if (eol === '\r\n') content = content.replace(/\n/g, '\r\n');
  return { ok: true, content };
}

/** Resolve a model-supplied relative path against the project root without any
 *  Node APIs. Rejects paths that escape the root. macOS uses "/" separators. */
export function joinProjectPath(root: string, rel: string): { abs: string } | { error: string } {
  const cleaned = rel.replace(/^(\.?\/)+/, '');
  const parts: string[] = [];
  for (const seg of cleaned.split('/')) {
    if (seg === '' || seg === '.') continue;
    if (seg === '..') {
      if (parts.length === 0) return { error: `Path escapes the project root: ${rel}` };
      parts.pop();
      continue;
    }
    parts.push(seg);
  }
  return { abs: `${root.replace(/\/+$/, '')}/${parts.join('/')}` };
}
