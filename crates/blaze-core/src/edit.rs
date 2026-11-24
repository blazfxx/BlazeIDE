//! SEARCH/REPLACE edits (the `apply_diff` tool), ported from the UI's
//! `src/shared/applyDiff.ts` so the CLI, the phone and BlBots edit files
//! exactly like the IDE does. Both implementations are checked against the
//! same cases in `tests/fixtures/apply_diff_cases.json`.
//!
//! Diff format, one or more blocks per diff:
//!
//! ```text
//! <<<<<<< SEARCH
//! :start_line:42        (optional hint)
//! -------               (optional separator)
//! <text to find>
//! =======
//! <replacement text>
//! >>>>>>> REPLACE
//! ```
//!
//! An empty SEARCH body means "create or replace the whole file". Blocks
//! apply in order against the running content.

struct Block {
    start_line: usize,
    search: String,
    replace: String,
}

fn is_open(l: &str) -> bool {
    // ^<{3,}\s*SEARCH\s*>?\s*$
    let t = l.trim_start_matches('<');
    if l.len() - t.len() < 3 {
        return false;
    }
    let t = t.trim_start();
    let Some(rest) = t.strip_prefix("SEARCH") else { return false };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('>').unwrap_or(rest);
    rest.trim().is_empty()
}

fn is_sep(l: &str) -> bool {
    let t = l.trim_start_matches('=');
    l.len() - t.len() >= 3 && t.trim().is_empty()
}

fn is_close(l: &str) -> bool {
    let t = l.trim_start_matches('>');
    if l.len() - t.len() < 3 {
        return false;
    }
    let t = t.trim_start();
    t.strip_prefix("REPLACE").is_some_and(|r| r.trim().is_empty())
}

fn is_fence(l: &str) -> bool {
    let t = l.trim_start_matches('-');
    l.len() - t.len() >= 3 && t.trim().is_empty()
}

fn start_line(l: &str) -> Option<usize> {
    let rest = l.strip_prefix(":start_line:")?.trim();
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn parse_blocks(diff: &str) -> Result<Vec<Block>, String> {
    let norm = diff.replace("\r\n", "\n");
    let lines: Vec<&str> = norm.split('\n').collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_open(lines[i]) {
            i += 1;
            continue;
        }
        i += 1;
        let mut start = 0;
        if i < lines.len() {
            if let Some(n) = start_line(lines[i]) {
                start = n;
                i += 1;
            }
        }
        if i < lines.len() && is_fence(lines[i]) {
            i += 1;
        }
        let mut search = Vec::new();
        while i < lines.len() && !is_sep(lines[i]) && !is_close(lines[i]) {
            search.push(lines[i]);
            i += 1;
        }
        if i >= lines.len() || !is_sep(lines[i]) {
            return Err("Malformed diff: missing \"=======\" between SEARCH and REPLACE.".into());
        }
        i += 1;
        let mut replace = Vec::new();
        while i < lines.len() && !is_close(lines[i]) {
            replace.push(lines[i]);
            i += 1;
        }
        if i >= lines.len() {
            return Err("Malformed diff: missing \">>>>>>> REPLACE\" marker.".into());
        }
        i += 1;
        blocks.push(Block { start_line: start, search: search.join("\n"), replace: replace.join("\n") });
    }
    if blocks.is_empty() {
        return Err("No SEARCH/REPLACE blocks found in the diff.".into());
    }
    Ok(blocks)
}

fn find_exact(content: &str, search: &str, from: usize, start_line: usize) -> Option<usize> {
    if start_line > 0 {
        let mut off = Some(0usize);
        for _ in 1..start_line {
            off = off.and_then(|o| content[o..].find('\n').map(|nl| o + nl + 1));
            if off.is_none() {
                break;
            }
        }
        if let Some(o) = off {
            if content[o..].starts_with(search) {
                return Some(o);
            }
        }
    }
    if from > content.len() {
        return None;
    }
    content[from..].find(search).map(|i| from + i)
}

fn find_trimmed(content: &str, search: &str, from: usize) -> Option<(usize, usize)> {
    let c_lines: Vec<&str> = content.split('\n').collect();
    let s_lines: Vec<&str> = search.split('\n').map(str::trim).collect();
    let mut offsets = Vec::with_capacity(c_lines.len());
    let mut acc = 0;
    for l in &c_lines {
        offsets.push(acc);
        acc += l.len() + 1;
    }
    let mut found: Option<usize> = None;
    let mut i = 0;
    while i + s_lines.len() <= c_lines.len() {
        if offsets[i] >= from && (0..s_lines.len()).all(|j| c_lines[i + j].trim() == s_lines[j]) {
            if found.is_some() {
                return None; // ambiguous: refuse to guess
            }
            found = Some(i);
        }
        i += 1;
    }
    let m = found?;
    let last = m + s_lines.len() - 1;
    Some((offsets[m], offsets[last] + c_lines[last].len()))
}

/// Apply one or more SEARCH/REPLACE blocks to `original`. CRLF files stay CRLF.
pub fn apply_diff(original: &str, diff: &str) -> Result<String, String> {
    let blocks = parse_blocks(diff)?;
    let crlf = original.contains("\r\n");
    let mut content = original.replace("\r\n", "\n");
    let mut cursor = 0usize;
    for (b, block) in blocks.iter().enumerate() {
        let search = block.search.replace("\r\n", "\n");
        let replace = block.replace.replace("\r\n", "\n");
        if search.trim().is_empty() {
            content = replace;
            cursor = content.len();
            continue;
        }
        if let Some(at) = find_exact(&content, &search, cursor, block.start_line) {
            content = format!("{}{}{}", &content[..at], replace, &content[at + search.len()..]);
            cursor = at + replace.len();
            continue;
        }
        if let Some((start, end)) = find_trimmed(&content, &search, cursor) {
            content = format!("{}{}{}", &content[..start], replace, &content[end..]);
            cursor = start + replace.len();
            continue;
        }
        return Err(format!(
            "SEARCH block {} did not match the file. Re-read the file to get its current content, then retry with an exact SEARCH.",
            b + 1
        ));
    }
    if crlf {
        content = content.replace('\n', "\r\n");
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        original: String,
        diff: String,
        ok: bool,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    /// The same cases the TypeScript implementation produced (see
    /// tests/fixtures/make_apply_diff_cases.mjs).
    #[test]
    fn matches_the_typescript_engine() {
        let raw = include_str!("../../../tests/fixtures/apply_diff_cases.json");
        let cases: Vec<Case> = serde_json::from_str(raw).unwrap();
        assert!(cases.len() >= 15);
        for c in cases {
            let got = apply_diff(&c.original, &c.diff);
            match (c.ok, got) {
                (true, Ok(s)) => assert_eq!(Some(s), c.content, "case {}", c.name),
                (false, Err(e)) => assert_eq!(Some(e), c.error, "case {}", c.name),
                (want, got) => panic!("case {}: expected ok={want}, got {got:?}", c.name),
            }
        }
    }
}
