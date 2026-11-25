//! Drawing. opencode-style: a shaded title block with tokens and cost, your
//! messages and the prompt in shaded blocks with an orange left bar, quiet
//! tool lines, key hints along the bottom.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::commands::COMMANDS;
use super::{onboard_models, App, Entry, Onboard, Overlay, Pending, PickKind, Target, ToolState};

const ACCENT: Color = Color::Indexed(208);
const SHADE: Color = Color::Indexed(235);
const SHADE2: Color = Color::Indexed(237);
const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

const LOGO: [&str; 6] = [
    "██████╗ ██╗      █████╗ ███████╗███████╗",
    "██╔══██╗██║     ██╔══██╗╚══███╔╝██╔════╝",
    "██████╔╝██║     ███████║  ███╔╝ █████╗  ",
    "██╔══██╗██║     ██╔══██║ ███╔╝  ██╔══╝  ",
    "██████╔╝███████╗██║  ██║███████╗███████╗",
    "╚═════╝ ╚══════╝╚═╝  ╚═╝╚══════╝╚══════╝",
];

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn pad(line: Line<'static>, width: usize, bg: Color) -> Line<'static> {
    let w = line.width();
    let mut l = line;
    if w < width {
        l.spans.push(Span::raw(" ".repeat(width - w)));
    }
    l.style(Style::default().bg(bg))
}

/// `inline code` and **bold** inside a line of model text.
fn rich(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            out.push(Span::styled(part.to_string(), base.fg(Color::Indexed(180))));
            continue;
        }
        for (j, b) in part.split("**").enumerate() {
            if !b.is_empty() {
                out.push(Span::styled(b.to_string(), if j % 2 == 1 { base.add_modifier(Modifier::BOLD) } else { base }));
            }
        }
    }
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    textwrap::wrap(text, width.max(10)).into_iter().map(|c| c.into_owned()).collect()
}

fn transcript(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let inner = width.saturating_sub(4);
    for e in &app.entries {
        match e {
            Entry::User(t) => {
                out.push(Line::default());
                out.push(pad(Line::from(Span::styled("▌", Style::default().fg(ACCENT))), width, SHADE));
                for raw in t.split('\n') {
                    for w in wrap(raw, inner) {
                        out.push(pad(Line::from(vec![Span::styled("▌ ", Style::default().fg(ACCENT)), Span::styled(w, Style::default().fg(Color::White))]), width, SHADE));
                    }
                }
                out.push(pad(Line::from(Span::styled("▌", Style::default().fg(ACCENT))), width, SHADE));
            }
            Entry::Assistant(t) => {
                out.push(Line::default());
                let mut code = false;
                for raw in t.trim_end().split('\n') {
                    if raw.trim_start().starts_with("```") {
                        code = !code;
                        continue;
                    }
                    if code {
                        out.push(pad(Line::from(Span::styled(format!("  {raw}"), Style::default().fg(Color::Gray))), width, SHADE2));
                        continue;
                    }
                    let (style, body) = if let Some(h) = raw.strip_prefix('#') {
                        (Style::default().add_modifier(Modifier::BOLD), h.trim_start_matches('#').trim_start())
                    } else {
                        (Style::default(), raw)
                    };
                    for w in wrap(body, inner) {
                        let mut spans = vec![Span::raw("  ")];
                        spans.extend(rich(&w, style));
                        out.push(Line::from(spans));
                    }
                }
            }
            Entry::Tool { summary, state } => {
                let (mark, st) = match state {
                    ToolState::Running => (SPIN[app.spinner % SPIN.len()], Style::default().fg(ACCENT)),
                    ToolState::Ok => ("→", dim()),
                    ToolState::Failed(_) => ("✗", Style::default().fg(Color::Red)),
                };
                let s: String = summary.chars().take(inner).collect();
                out.push(Line::from(vec![Span::raw("  "), Span::styled(format!("{mark} "), st), Span::styled(s, Style::default().fg(Color::Gray))]));
                if let ToolState::Failed(m) = state {
                    out.push(Line::from(Span::styled(format!("    {}", m.chars().take(inner).collect::<String>()), Style::default().fg(Color::Red))));
                }
            }
            Entry::File { rel, added, removed, created } => {
                out.push(Line::from(vec![
                    Span::styled(format!("  {} ", if *created { "+" } else { "~" }), dim()),
                    Span::raw(rel.clone()),
                    Span::styled(format!("  +{added}"), Style::default().fg(Color::Green)),
                    Span::styled(format!(" -{removed}"), Style::default().fg(Color::Red)),
                ]));
            }
            Entry::Notice(t) => {
                out.push(Line::default());
                for w in wrap(t, inner) {
                    out.push(Line::from(Span::styled(format!("  {w}"), dim())));
                }
            }
            Entry::Error(t) => {
                out.push(Line::default());
                for w in wrap(t, inner) {
                    out.push(Line::from(Span::styled(format!("  ! {w}"), Style::default().fg(Color::Red))));
                }
            }
        }
    }
    out
}

fn home(app: &App, area: Rect) -> Vec<Line<'static>> {
    let mut l: Vec<Line<'static>> = Vec::new();
    let w = area.width as usize;
    let top = (area.height as usize).saturating_sub(20) / 2;
    for _ in 0..top {
        l.push(Line::default());
    }
    let center = |s: String, style: Style| -> Line<'static> {
        let pad = w.saturating_sub(s.width()) / 2;
        Line::from(vec![Span::raw(" ".repeat(pad)), Span::styled(s, style)])
    };
    if w >= 46 {
        for row in LOGO {
            l.push(center(row.to_string(), Style::default().fg(ACCENT)));
        }
    } else {
        l.push(center("B L A Z E".into(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
    }
    l.push(Line::default());
    let who = app.s.account_name().unwrap_or_else(|| if app.s.key.is_some() { "signed in".into() } else { "not signed in · /login".into() });
    let project = app.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    l.push(center(format!("v{}  ·  {}  ·  {}", crate::VERSION, project, who), dim()));
    l.push(Line::default());
    for (k, d) in [("/rc", "continue on your phone"), ("/model", "pick a model"), ("/chats", "open a saved chat"), ("/bots", "talk to your BlBots"), ("/help", "all commands and keys")] {
        let s = format!("{k:<9}{d}");
        let pad = w.saturating_sub(32) / 2;
        l.push(Line::from(vec![Span::raw(" ".repeat(pad)), Span::styled(format!("{k:<9}"), Style::default().fg(ACCENT)), Span::styled(s[9..].to_string(), Style::default().fg(Color::Gray))]));
    }
    let recent = blaze_core::chats::list(&app.root);
    if !recent.is_empty() {
        l.push(Line::default());
        l.push(center("Recent chats (/chats)".into(), dim()));
        for c in recent.iter().take(3) {
            let t: String = c.title.chars().take(48).collect();
            l.push(center(t, Style::default().fg(Color::Gray)));
        }
    }
    l
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn panel(f: &mut Frame, r: Rect, lines: Vec<Line<'static>>) {
    f.render_widget(Clear, r);
    let block = Block::default().style(Style::default().bg(SHADE));
    let inner = Rect { x: r.x + 2, y: r.y + 1, width: r.width.saturating_sub(4), height: r.height.saturating_sub(2) };
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(SHADE)), inner);
}

fn title_line(t: &str) -> Line<'static> {
    Line::from(Span::styled(t.to_string(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
}

fn onboarding(f: &mut Frame, app: &App, step: Onboard) {
    let area = f.area();
    let r = centered(area, 74, 24);
    let mut l: Vec<Line<'static>> = Vec::new();
    let n = match step { Onboard::Welcome => 1, Onboard::SignIn => 2, Onboard::Model => 3, Onboard::Approval => 4 };
    l.push(Line::from(Span::styled(format!("Setup  {n} / 4"), dim())));
    l.push(Line::default());
    match step {
        Onboard::Welcome => {
            if r.width >= 46 {
                for row in LOGO {
                    l.push(Line::from(Span::styled(row.to_string(), Style::default().fg(ACCENT))));
                }
            }
            l.push(Line::default());
            l.push(Line::from("Blaze IDE's agent, in your terminal."));
            l.push(Line::from(Span::styled("It edits this project and runs commands when you allow it.", Style::default().fg(Color::Gray))));
            l.push(Line::from(Span::styled("Chats also open in Blaze IDE and on your phone (/rc).", Style::default().fg(Color::Gray))));
            l.push(Line::default());
            l.push(Line::from(vec![Span::styled("Enter", Style::default().fg(ACCENT)), Span::styled(" start   ", dim()), Span::styled("Esc", Style::default().fg(ACCENT)), Span::styled(" skip setup", dim())]));
        }
        Onboard::SignIn => {
            l.push(title_line("Sign in with BlazeAPI"));
            l.push(Line::default());
            l.push(Line::from(Span::styled("Requests are billed to your BlazeAPI balance. Your browser opens to sign in;", Style::default().fg(Color::Gray))));
            l.push(Line::from(Span::styled("this computer gets its own key, kept in your system keychain.", Style::default().fg(Color::Gray))));
            l.push(Line::default());
            if app.signing_in {
                l.push(Line::from(vec![Span::styled(format!("{} ", SPIN[app.spinner % SPIN.len()]), Style::default().fg(ACCENT)), Span::raw("Waiting for the browser...")]));
            }
            if let Some(Entry::Error(e)) = app.entries.last() {
                l.push(Line::from(Span::styled(e.clone(), Style::default().fg(Color::Red))));
            }
            l.push(Line::default());
            l.push(Line::from(vec![Span::styled("Enter", Style::default().fg(ACCENT)), Span::styled(" sign in   ", dim()), Span::styled("s", Style::default().fg(ACCENT)), Span::styled(" skip (blaze login --key later)", dim())]));
        }
        Onboard::Model => {
            l.push(title_line("Pick your default model"));
            l.push(Line::from(Span::styled("Change it any time with /model or Ctrl+L.", dim())));
            l.push(Line::default());
            for (i, (id, name, price)) in onboard_models(app).iter().enumerate() {
                let sel = i == app.onboard_sel;
                let mark = if sel { "› " } else { "  " };
                l.push(Line::from(vec![
                    Span::styled(mark.to_string(), Style::default().fg(ACCENT)),
                    Span::styled(format!("{name:<30}"), if sel { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    Span::styled(format!(" {price}"), dim()),
                    Span::styled(if id == &app.model { "  (current)".to_string() } else { String::new() }, dim()),
                ]));
            }
            l.push(Line::default());
            l.push(Line::from(vec![Span::styled("↑↓", Style::default().fg(ACCENT)), Span::styled(" choose   ", dim()), Span::styled("Enter", Style::default().fg(ACCENT)), Span::styled(" next", dim())]));
        }
        Onboard::Approval => {
            l.push(title_line("What may the agent do without asking?"));
            l.push(Line::from(Span::styled("Change it any time with /approval.", dim())));
            l.push(Line::default());
            let opts = [("Ask first", "every change and command waits for your y/n"), ("Approve edits", "edits files here, asks before commands (recommended)"), ("Full access", "never asks; use only in projects you can restore")];
            for (i, (name, d)) in opts.iter().enumerate() {
                let sel = i == app.onboard_sel;
                l.push(Line::from(vec![
                    Span::styled(if sel { "› " } else { "  " }.to_string(), Style::default().fg(ACCENT)),
                    Span::styled(format!("{name:<15}"), if sel { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) }),
                    Span::styled(d.to_string(), dim()),
                ]));
            }
            l.push(Line::default());
            l.push(Line::from(vec![Span::styled("Enter", Style::default().fg(ACCENT)), Span::styled(" finish", dim())]));
        }
    }
    f.render_widget(Block::default().style(Style::default().bg(Color::Reset)), area);
    panel(f, r, l);
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if let Some(step) = app.onboard {
        onboarding(f, app, step);
        return;
    }
    let width = area.width as usize;
    let input_rows = app.input.split('\n').count().clamp(1, 8) as u16;
    let pal = app.palette();
    let pal_h = pal.len().min(8) as u16;
    let pending_h: u16 = match &app.pending {
        Some(Pending::Approve { .. }) => 4,
        Some(Pending::Ask { questions, answers, .. }) => 4 + questions.get(answers.len()).map(|(_, o)| o.len().min(6)).unwrap_or(0) as u16,
        None => 0,
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3), Constraint::Length(pending_h), Constraint::Length(pal_h), Constraint::Length(input_rows + 3), Constraint::Length(1)])
        .split(area);

    // Title block.
    let mut stats = Vec::new();
    if app.tokens.0 + app.tokens.1 > 0 {
        stats.push(format!("{}", app.tokens.0 + app.tokens.1));
    }
    if let Some(p) = app.context_pct() {
        stats.push(format!("{p}%"));
    }
    if app.cost > 0.0 {
        stats.push(format!("(${:.2})", app.cost));
    }
    let right = stats.join("  ");
    let left = format!("  # {}", app.title());
    let gap = width.saturating_sub(left.width() + right.width() + 2);
    let head = vec![
        pad(Line::default(), width, SHADE),
        pad(Line::from(vec![Span::styled(left, Style::default().add_modifier(Modifier::BOLD)), Span::raw(" ".repeat(gap)), Span::styled(right, dim())]), width, SHADE),
        pad(Line::default(), width, SHADE),
    ];
    f.render_widget(Paragraph::new(head), rows[0]);

    // Transcript or home screen.
    if app.entries.is_empty() && matches!(app.target, Target::Project) {
        f.render_widget(Paragraph::new(home(app, rows[1])), rows[1]);
    } else {
        let lines = transcript(app, width);
        let h = rows[1].height as usize;
        let max_scroll = lines.len().saturating_sub(h);
        let scroll = app.scroll.min(max_scroll);
        let start = lines.len().saturating_sub(h + scroll);
        let end = (start + h).min(lines.len());
        f.render_widget(Paragraph::new(lines[start..end].to_vec()), rows[1]);
    }

    // Approval / question.
    match &app.pending {
        Some(Pending::Approve { summary, .. }) => {
            let l = vec![
                pad(Line::default(), width, SHADE2),
                pad(Line::from(vec![Span::styled("  △ Allow? ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)), Span::raw(summary.chars().take(width.saturating_sub(14)).collect::<String>())]), width, SHADE2),
                pad(Line::from(vec![Span::styled("    y", Style::default().fg(ACCENT)), Span::styled(" yes   ", dim()), Span::styled("a", Style::default().fg(ACCENT)), Span::styled(" always for this tool   ", dim()), Span::styled("n", Style::default().fg(ACCENT)), Span::styled(" no", dim())]), width, SHADE2),
                pad(Line::default(), width, SHADE2),
            ];
            f.render_widget(Paragraph::new(l), rows[2]);
        }
        Some(Pending::Ask { questions, answers, .. }) => {
            if let Some((q, opts)) = questions.get(answers.len()) {
                let mut l = vec![pad(Line::default(), width, SHADE2), pad(Line::from(Span::styled(format!("  ? {q}"), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))), width, SHADE2)];
                for (i, o) in opts.iter().take(6).enumerate() {
                    l.push(pad(Line::from(vec![Span::styled(format!("    {}", i + 1), Style::default().fg(ACCENT)), Span::raw(format!(" {o}"))]), width, SHADE2));
                }
                l.push(pad(Line::from(Span::styled("    type a number or your own answer, then Enter", dim())), width, SHADE2));
                l.push(pad(Line::default(), width, SHADE2));
                f.render_widget(Paragraph::new(l), rows[2]);
            }
        }
        None => {}
    }

    // Command menu.
    if !pal.is_empty() {
        let sel = app.palette_sel.min(pal.len() - 1);
        let skip = sel.saturating_sub(7);
        let l: Vec<Line<'static>> = pal
            .iter()
            .enumerate()
            .skip(skip)
            .take(8)
            .map(|(i, c)| {
                let on = i == sel;
                let name = format!("  /{:<10}", c.name);
                let args = format!("{:<24}", c.args);
                pad(
                    Line::from(vec![
                        Span::styled(name, if on { Style::default().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) }),
                        Span::styled(args, dim()),
                        Span::styled(c.desc.to_string(), Style::default().fg(if on { Color::White } else { Color::Gray })),
                    ]),
                    width,
                    if on { SHADE2 } else { SHADE },
                )
            })
            .collect();
        f.render_widget(Paragraph::new(l), rows[3]);
    }

    // Prompt block.
    let bar = || Span::styled("▌ ", Style::default().fg(ACCENT));
    let mut pl = vec![pad(Line::from(bar()), width, SHADE)];
    let placeholder = app.input.is_empty();
    if placeholder {
        let hint = match &app.target {
            Target::Bot { name, .. } => format!("Message {name}..."),
            Target::Project => "Ask anything, or / for commands".into(),
        };
        pl.push(pad(Line::from(vec![bar(), Span::styled(hint, dim())]), width, SHADE));
    } else {
        for line in app.input.split('\n').take(8) {
            pl.push(pad(Line::from(vec![bar(), Span::raw(line.to_string())]), width, SHADE));
        }
    }
    let (mode_label, mode_color) = match app.mode.as_str() {
        "ask" => ("Ask", Color::Cyan),
        "plan" => ("Plan", Color::Yellow),
        _ => ("Agent", ACCENT),
    };
    let target = match &app.target {
        Target::Bot { name, .. } => format!("  BlBot {name}"),
        Target::Project => format!("  {}", app.model_label()),
    };
    pl.push(pad(
        Line::from(vec![bar(), Span::styled(mode_label, Style::default().fg(mode_color).add_modifier(Modifier::BOLD)), Span::styled(target, Style::default().fg(Color::White)), Span::styled(format!("  {}", app.approval.label()), dim())]),
        width,
        SHADE,
    ));
    f.render_widget(Paragraph::new(pl), rows[4]);
    if app.overlay.is_none() && !matches!(app.pending, Some(Pending::Approve { .. })) {
        let before = &app.input[..app.cursor];
        let row = before.matches('\n').count() as u16;
        let col = before.rsplit('\n').next().unwrap_or("").width() as u16;
        f.set_cursor_position((rows[4].x + 2 + col.min(rows[4].width.saturating_sub(3)), rows[4].y + 1 + row.min(7)));
    }

    // Footer.
    let left = if app.is_running() {
        vec![Span::styled(format!(" {} ", SPIN[app.spinner % SPIN.len()]), Style::default().fg(ACCENT)), Span::styled(format!("working {}s  ", app.elapsed()), Style::default().fg(Color::Gray)), Span::styled("esc", Style::default().fg(Color::White)), Span::styled(" interrupt", dim())]
    } else if !app.status.is_empty() {
        vec![Span::styled(format!(" {}", app.status), Style::default().fg(Color::Gray))]
    } else {
        vec![Span::raw(" ")]
    };
    let mut right = Vec::new();
    if app.rc.is_some() {
        right.push(Span::styled("● phone  ", Style::default().fg(Color::Green)));
    } else if app.rc_busy {
        right.push(Span::styled("◌ phone  ", Style::default().fg(Color::Yellow)));
    }
    for (k, d) in [("tab", "mode"), ("ctrl+l", "model"), ("ctrl+r", "phone"), ("/", "commands")] {
        right.push(Span::styled(k, Style::default().fg(Color::White)));
        right.push(Span::styled(format!(" {d}  "), dim()));
    }
    let lw: usize = left.iter().map(|s| s.width()).sum();
    let rw: usize = right.iter().map(|s| s.width()).sum();
    let mut foot = left;
    if lw + rw < width {
        foot.push(Span::raw(" ".repeat(width - lw - rw)));
        foot.extend(right);
    }
    f.render_widget(Paragraph::new(Line::from(foot)), rows[5]);

    // Overlays.
    if let Some(ov) = &app.overlay {
        match ov {
            Overlay::Help => {
                let key = |k: &str| Span::styled(format!("{k:<11}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
                let mut l = vec![
                    Line::from(vec![Span::styled("Help", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled("   type / anywhere to open the command menu", dim())]),
                    Line::default(),
                ];
                let mut group = "";
                for c in COMMANDS {
                    if c.group != group {
                        if !group.is_empty() {
                            l.push(Line::default());
                        }
                        group = c.group;
                        l.push(Line::from(Span::styled(group.to_uppercase(), dim().add_modifier(Modifier::BOLD))));
                    }
                    l.push(Line::from(vec![
                        Span::styled(format!("  /{:<10}", c.name), Style::default().fg(ACCENT)),
                        Span::styled(c.desc.to_string(), Style::default().fg(Color::White)),
                        Span::styled(if c.args.is_empty() { String::new() } else { format!("  {}", c.args) }, dim()),
                    ]));
                }
                l.push(Line::default());
                l.push(Line::from(Span::styled("KEYS", dim().add_modifier(Modifier::BOLD))));
                let keys = [("enter", "send"), ("alt+enter", "new line"), ("esc", "stop the turn"), ("tab", "Agent / Plan / Ask"), ("ctrl+l", "pick a model"), ("ctrl+p", "command menu"), ("ctrl+r", "phone (/rc)"), ("ctrl+c", "clear / stop / quit"), ("pgup pgdn", "scroll"), ("↑ ↓", "message history")];
                for pair in keys.chunks(2) {
                    let mut spans = vec![Span::raw("  "), key(pair[0].0), Span::styled(format!("{:<24}", pair[0].1), Style::default().fg(Color::Gray))];
                    if let Some((k, d)) = pair.get(1) {
                        spans.push(key(k));
                        spans.push(Span::styled(d.to_string(), Style::default().fg(Color::Gray)));
                    }
                    l.push(Line::from(spans));
                }
                l.push(Line::default());
                l.push(Line::from(Span::styled("esc to close", dim())));
                let h = l.len() as u16 + 2;
                panel(f, centered(area, 84, h), l);
            }
            Overlay::Info(t, lines) => {
                let mut l = vec![title_line(t), Line::default()];
                l.extend(lines.iter().map(|x| Line::from(Span::styled(x.clone(), Style::default().fg(Color::Gray)))));
                l.push(Line::default());
                l.push(Line::from(Span::styled("esc to close", dim())));
                let h = l.len() as u16 + 2;
                panel(f, centered(area, 80, h), l);
            }
            Overlay::Picker(p) => {
                let what = match p.kind {
                    PickKind::Model => "Model",
                    PickKind::Chats => "Chats",
                    PickKind::Bots => "BlBots",
                };
                let r = centered(area, 86, 22);
                let vis = p.visible();
                let sel = p.sel.min(vis.len().saturating_sub(1));
                let rows_avail = r.height.saturating_sub(6) as usize;
                let skip = sel.saturating_sub(rows_avail.saturating_sub(1));
                let mut l = vec![
                    Line::from(vec![Span::styled(format!("{what}  "), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(format!("{}▏", p.filter), Style::default().fg(Color::White)), Span::styled(format!("   {} found", vis.len()), dim())]),
                    Line::default(),
                ];
                for (i, it) in vis.iter().enumerate().skip(skip).take(rows_avail) {
                    let on = i == sel;
                    let label: String = it.label.chars().take(44).collect();
                    let style = if on { Style::default().fg(Color::Black).bg(ACCENT) } else { Style::default().fg(Color::White) };
                    l.push(Line::from(vec![Span::styled(format!(" {label:<44} "), style), Span::styled(format!(" {}", it.detail), dim())]));
                }
                l.push(Line::default());
                l.push(Line::from(Span::styled("type to filter · ↑↓ choose · enter pick · esc close", dim())));
                panel(f, r, l);
            }
            Overlay::Password(pw) => {
                let field = |label: &str, v: &str, on: bool| {
                    Line::from(vec![
                        Span::styled(format!("{label:<14}"), Style::default().fg(if on { Color::White } else { Color::Gray })),
                        Span::styled(format!("{}{}", "•".repeat(v.chars().count()), if on { "▏" } else { "" }), Style::default().fg(ACCENT)),
                    ])
                };
                let mut l = vec![
                    title_line("Remote connect: choose a phone password"),
                    Line::default(),
                    Line::from(Span::styled("Your phone signs in with this (or a one-time QR code). Only a hash is stored.", Style::default().fg(Color::Gray))),
                    Line::default(),
                    field("Password", &pw.a, !pw.second),
                    field("Again", &pw.b, pw.second),
                    Line::default(),
                ];
                if !pw.err.is_empty() {
                    l.push(Line::from(Span::styled(pw.err.clone(), Style::default().fg(Color::Red))));
                }
                l.push(Line::from(Span::styled("at least 10 characters · enter next · esc cancel", dim())));
                panel(f, centered(area, 80, 13), l);
            }
            Overlay::Rc => {
                let mut l = vec![title_line("Remote connect"), Line::default()];
                if app.rc_busy {
                    l.push(Line::from(vec![Span::styled(format!("{} ", SPIN[app.spinner % SPIN.len()]), Style::default().fg(ACCENT)), Span::raw("Starting...")]));
                    for x in app.rc_log.iter().rev().take(4).rev() {
                        l.push(Line::from(Span::styled(x.clone(), dim())));
                    }
                } else if let Some(r) = &app.rc {
                    for (what, url) in &r.urls {
                        l.push(Line::from(vec![Span::styled(url.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)), Span::styled(format!("  {what}"), dim())]));
                    }
                    l.push(Line::default());
                    let qr = app.rc_qr.clone().unwrap_or_default();
                    let qr_h = qr.lines().count() as u16;
                    let qr_w = qr.lines().next().map(|x| x.chars().count()).unwrap_or(0) as u16;
                    if qr_h + 10 <= area.height && qr_w + 6 <= area.width {
                        l.push(Line::from(Span::styled("Scan with your phone's camera to sign in (one-time, 5 minutes):", Style::default().fg(Color::Gray))));
                        for q in qr.lines() {
                            l.push(Line::from(Span::styled(q.to_string(), Style::default().fg(Color::White).bg(Color::Black))));
                        }
                    } else {
                        l.push(Line::from(Span::styled("Make the terminal bigger to see the QR code, or open the address and use your password.", Style::default().fg(Color::Yellow))));
                    }
                    l.push(Line::default());
                    l.push(Line::from(Span::styled("Phones can continue chats, approve actions and talk to BlBots while this window stays open.", Style::default().fg(Color::Gray))));
                } else {
                    for x in app.rc_log.iter().rev().take(5).rev() {
                        l.push(Line::from(Span::styled(x.clone(), Style::default().fg(Color::Gray))));
                    }
                }
                l.push(Line::default());
                l.push(Line::from(Span::styled("n new QR code · x turn off · esc close", dim())));
                let h = l.len() as u16 + 2;
                let w = l.iter().map(|x| x.width() as u16).max().unwrap_or(40) + 6;
                panel(f, centered(area, w.max(60), h), l);
            }
        }
    }
}
