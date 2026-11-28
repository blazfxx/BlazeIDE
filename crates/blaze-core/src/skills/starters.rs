//! Starter skills that ship with Blaze (our own text). Adding one puts an
//! editable copy in the library.

/// (name, SKILL.md)
pub const STARTERS: &[(&str, &str)] = &[
    ("code-review", r#"---
name: code-review
description: "Review code changes for bugs, security problems and risky edges. Use when asked to review code, a diff, a pull request, or to check work before shipping."
---

# Code review

1. Find out what changed (`git diff`, `git diff --staged`, or the files named) and what it is meant to do.
2. Read the surrounding code, not only the changed lines: callers, error paths, tests.
3. Look in this order: wrong behaviour, data loss, security (input handling, secrets, injection, permissions), concurrency, error handling, performance that will matter, then readability.
4. Check each suspected problem against the code before reporting it. Say how it would fail, with a concrete input or sequence.
5. Report findings most serious first: file and line, what goes wrong, a suggested fix. If you found nothing important, say so plainly. Do not pad the list with style opinions.

Do not change files unless asked.
"#),
    ("debugging", r#"---
name: debugging
description: "Find the real cause of a bug, crash, failing build or failing test before changing code. Use whenever something is broken or behaves unexpectedly."
---

# Debugging

1. Reproduce it. Run the failing command or test and read the whole error, from the first one.
2. State what you expected and what happened.
3. List the possible causes, then test the most likely one with the cheapest check (a log line, a small script, reading the code path). Change one thing at a time.
4. Do not "fix" symptoms: no swallowing errors, longer timeouts or retries until you know why it fails.
5. After the fix, run the failing case again and the nearby tests.
6. Tell the user the cause in a sentence or two, what you changed, and how you checked it. If you could not confirm the cause, say that.
"#),
    ("write-tests", r#"---
name: write-tests
description: "Write useful tests for existing or new code. Use when asked to add tests, raise coverage, or protect a bug fix against coming back."
---

# Writing tests

1. Find how this project already tests (runner, folder, naming, helpers) and follow it.
2. Test behaviour through the public interface, not private details.
3. Cover: the normal case, the edges (empty, large, boundary values), invalid input, and failure of anything external.
4. For a bug fix, first write the test that fails without the fix.
5. Keep tests independent: no shared state, no real network, no sleeping to wait for things.
6. Run them. A test you have not seen fail and pass is not finished.
"#),
    ("commit-and-pr", r#"---
name: commit-and-pr
description: "Write clear git commit messages and pull request descriptions. Use when asked to commit, prepare a PR, or summarise changes."
---

# Commits and pull requests

1. Look at `git status` and the diff. Never commit secrets, local settings or build output.
2. One commit = one logical change. Suggest splitting unrelated changes.
3. Subject: under 72 characters, what the change does, in the imperative ("Fix login redirect").
4. Body (when useful): why it was needed and anything a reviewer should know.
5. A PR description says what changed, why, how it was tested, and what could break.
6. Commit or push only when the user asks.
"#),
    ("frontend-design", r#"---
name: frontend-design
description: "Design and build a web or app interface that looks intentional and works on phones. Use when creating or restyling pages, components, dashboards or landing pages."
---

# Interface design

- Decide the look first: one type scale, a small palette with one accent, one spacing scale, one corner radius. Reuse them.
- Match the existing design system if the project has one.
- Hierarchy: each screen has one main action. Size, weight and space show importance before colour does.
- Avoid the generic AI look: gradient headlines, glowing blobs, emoji as icons, three identical feature cards.
- Every state: loading, empty, error, long text, disabled.
- Accessible: text contrast at least 4.5:1, visible focus, labels on inputs, real buttons and links.
- Check at 390 px wide and at desktop width. No sideways scrolling, nothing clipped.
- If a preview or browser tool is available, look at the result and fix what you see before finishing.
"#),
    ("security-check", r#"---
name: security-check
description: "Check a project for common security problems. Use when asked to audit, harden or check the security of code the user owns."
---

# Security check

Work only on the user's own project.

1. Secrets: keys or passwords in the code, config, git history or client bundles.
2. Input: SQL, shell, path and HTML injection; unsafe deserialisation; unchecked uploads.
3. Access: every server route checks who is asking AND whether they may touch that record.
4. Sessions: password hashing, cookie flags, CSRF, rate limits on sign-in.
5. Dependencies: run the ecosystem's audit command.
6. Report by severity with file and line, how it could be abused in one sentence, and the fix. Do not claim the project is secure; say what you checked and what you did not.
"#),
    ("write-docs", r#"---
name: write-docs
description: "Write or improve a README, setup guide or code documentation. Use when asked for docs, a README, or instructions."
---

# Writing docs

1. Read the code and run the commands you document; do not guess flags or file names.
2. Start with what it is and what it is for, in two sentences.
3. Then the shortest path to running it: requirements, install, run, one example with its output.
4. Then configuration, common tasks, troubleshooting.
5. Short sentences, one idea each. Real commands in code blocks. No marketing words.
6. Say what does not work yet.
"#),
    ("plan-feature", r#"---
name: plan-feature
description: "Plan a larger feature or refactor before writing code. Use when a request spans several files or has unclear requirements."
---

# Planning

1. Read the parts of the code the work touches before proposing anything.
2. Restate the goal and what is out of scope. Ask only the questions whose answers change the plan.
3. Give the approach: the files to change, the data and interfaces, the order of steps, each step small enough to check on its own.
4. Name the risks and how each step will be verified.
5. Prefer the smallest change that solves the problem. Reuse what exists.
"#),
];
