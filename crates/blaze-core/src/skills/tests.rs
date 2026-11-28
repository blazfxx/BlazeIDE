use super::*;
use crate::agent::AgentConfig;

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn reads_frontmatter_forms() {
    let (n, d, body) = parse("---\nname: pdf-tools\ndescription: \"Work with \\\"PDF\\\" files\"\nlicense: MIT\n---\n# Body\n");
    assert_eq!((n.as_deref(), d.as_deref()), (Some("pdf-tools"), Some("Work with \"PDF\" files")));
    assert_eq!(body.trim(), "# Body");
    let (_, d, _) = parse("---\r\nname: a\r\ndescription: >-\r\n  Folded over\r\n  two lines\r\nallowed-tools: Read\r\n---\r\nx");
    assert_eq!(d.as_deref(), Some("Folded over two lines"));
    let (_, d, _) = parse("---\ndescription: plain value that\n  continues\n---\nx");
    assert_eq!(d.as_deref(), Some("plain value that continues"));
    assert_eq!(parse("# No frontmatter\ntext").0, None);
}

#[test]
fn starters_are_valid_and_editable() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(tmp.path());
    for (name, _) in STARTERS {
        assert_eq!(lib.add_starter(name).unwrap(), *name);
    }
    assert_eq!(lib.list().len(), STARTERS.len());
    assert!(lib.list().iter().all(|s| s.chat && s.source == "Starter" && s.description.len() > 40));
    assert!(lib.add_starter("debugging").is_err(), "no silent overwrite");
    // Rename through the editor keeps the chat switch.
    lib.set_chat(&["debugging".into()], false).unwrap();
    let text = lib.text("debugging").unwrap().replace("name: debugging", "name: My Debugging");
    assert_eq!(lib.save(Some("debugging"), &text).unwrap(), "my-debugging");
    let s = lib.list().into_iter().find(|s| s.name == "my-debugging").unwrap();
    assert!(!s.chat);
    assert!(lib.folder("debugging").is_none());
    assert!(lib.save(None, "no frontmatter").is_err());
    assert!(lib.save(None, "---\nname: x\n---\nno description").is_err());
    lib.remove("my-debugging").unwrap();
    assert!(lib.folder("my-debugging").is_none());
}

#[test]
fn finds_and_imports_other_tools() {
    let home = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    let h = home.path();
    put(&h.join(".claude/skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: PDF work\n---\nUse scripts/run.py");
    put(&h.join(".claude/skills/pdf/scripts/run.py"), "print(1)");
    put(&h.join(".claude/skills/pdf/.env"), "SECRET=1");
    put(&h.join(".claude/skills/pdf/server.pem"), "key");
    put(&h.join(".claude/plugins/cache/market/plug/1.0/skills/deploy/SKILL.md"), "---\nname: deploy\ndescription: Ship it\n---\nsteps");
    put(&h.join(".claude/commands/git/fix-issue.md"), "Fix issue $ARGUMENTS carefully.");
    put(&h.join(".codex/skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: Another PDF skill\n---\ndifferent");
    put(&h.join(".codex/skills/.system/hidden/SKILL.md"), "---\nname: hidden\ndescription: built in\n---\n");
    put(&h.join(".codex/prompts/explain.md"), "---\ndescription: Explain code\n---\nExplain this.");
    put(&h.join(".config/opencode/skill/lint/SKILL.md"), "---\nname: lint\ndescription: Lint\n---\nx");
    // The same skill installed twice is listed once.
    put(&h.join(".agents/skills/lint/SKILL.md"), "---\nname: lint\ndescription: Lint\n---\nx");

    let lib = Library::open(cfg.path());
    let found = lib.discover(h, None);
    let names: Vec<(&str, &str)> = found.iter().map(|f| (f.name.as_str(), f.source.as_str())).collect();
    assert_eq!(
        names,
        vec![("pdf", "Claude Code"), ("fix-issue", "Claude Code command"), ("deploy", "Claude plugin"), ("pdf", "Codex"), ("explain", "Codex prompt"), ("lint", "OpenCode")]
    );
    assert!(found.iter().all(|f| f.status == "new"));

    let added = lib.import(&found, false).unwrap();
    assert_eq!(added, vec!["pdf", "fix-issue", "deploy", "pdf-2", "explain", "lint"]);
    assert_eq!(lib.files("pdf"), vec!["SKILL.md", "scripts/run.py"], "keys and .env stay behind");
    assert_eq!(parse(&lib.text("pdf-2").unwrap()).0.as_deref(), Some("pdf-2"));
    let cmd = lib.text("fix-issue").unwrap();
    assert!(cmd.contains("name: fix-issue") && cmd.contains("slash command") && cmd.contains("$ARGUMENTS"));
    assert_eq!(lib.list().iter().find(|s| s.name == "deploy").unwrap().source, "Claude plugin");
    let again = lib.discover(h, None);
    assert_eq!(again.iter().filter(|f| f.status == "same").count(), 5);
    assert_eq!(again.iter().filter(|f| f.status == "differs").count(), 1, "the second pdf");
    // Replace takes the newer text instead of adding another copy.
    put(&h.join(".claude/skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: PDF work v2\n---\nnew");
    let one: Vec<Found> = lib.discover(h, None).into_iter().filter(|f| f.source == "Claude Code").collect();
    assert_eq!(lib.import(&one, true).unwrap(), vec!["pdf"]);
    assert!(lib.text("pdf").unwrap().contains("v2"));
}

#[test]
fn use_skill_cannot_leave_the_skill() {
    let home = tempfile::tempdir().unwrap();
    let cfg = tempfile::tempdir().unwrap();
    put(&home.path().join("secret.txt"), "TOP SECRET");
    let dir = home.path().join(".claude/skills/s");
    put(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\nread ref.md");
    put(&dir.join("ref.md"), "reference text");
    #[cfg(unix)]
    std::os::unix::fs::symlink(home.path().join("secret.txt"), dir.join("link.txt")).unwrap();
    let lib = Library::open(cfg.path());
    lib.import(&lib.discover(home.path(), None), false).unwrap();
    assert_eq!(lib.files("s"), vec!["SKILL.md", "ref.md"]);
    let allowed = vec!["s".to_string()];
    let call = |a: &str| tool::run(&lib, &allowed, a);
    assert!(call(r#"{"name":"s"}"#).contains("read ref.md"));
    assert_eq!(call(r#"{"name":"s","file":"ref.md"}"#), "reference text");
    for bad in [r#"{"name":"s","file":"../../secret.txt"}"#, r#"{"name":"s","file":"link.txt"}"#, r#"{"name":"s","file":"/etc/passwd"}"#, r#"{"name":"../.."}"#, r#"{"name":"other"}"#] {
        assert!(call(bad).starts_with("Error"), "{bad}");
    }
}

#[test]
fn turns_get_the_list_and_the_tool() {
    let cfg_dir = tempfile::tempdir().unwrap();
    let lib = Library::open(cfg_dir.path());
    lib.add_starter("debugging").unwrap();
    lib.add_starter("code-review").unwrap();
    lib.set_chat(&["code-review".into()], false).unwrap();

    let mut c = AgentConfig::new(cfg_dir.path().into(), "m", "agent");
    c.extra_system = Some("base".into());
    tool::for_chat(&mut c, cfg_dir.path());
    let sys = c.extra_system.unwrap();
    assert!(sys.starts_with("base") && sys.contains("- debugging:") && !sys.contains("- code-review:"));
    let tools = c.extra_tools.unwrap();
    assert!(tools.handles("use_skill"));
    assert_eq!(tools.schemas().len(), 1);

    let mut b = AgentConfig::new(cfg_dir.path().into(), "m", "agent");
    tool::for_bot(&mut b, cfg_dir.path(), &["code-review".into(), "gone".into()]);
    assert!(b.extra_system.unwrap().contains("- code-review:"));

    let mut ask = AgentConfig::new(cfg_dir.path().into(), "m", "ask");
    tool::for_chat(&mut ask, cfg_dir.path());
    assert!(ask.extra_tools.is_none());

    // A huge library keeps the per-message list bounded.
    let many: Vec<Skill> = (0..400).map(|i| Skill { name: format!("skill-{i}"), description: "d".repeat(900), source: "You".into(), chat: true, files: 1, bytes: 1 }).collect();
    assert!(index(&many).unwrap().len() < 16_000);
}
