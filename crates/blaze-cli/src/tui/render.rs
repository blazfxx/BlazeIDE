//! Drawing the chat screen. Monochrome like the IDE, with a little colour for
//! state (green ok, red errors, yellow waiting on you).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{App, Entry, Pending, ToolState};

const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn wrap_into(out: &mut Vec<Line<'static>>, text: &str, width: usize, first: &str, rest: &str, style: Style) {
    let w = width.saturating_sub(first.width()).max(10);
    let mut first_line = true;
    let mut in_code = false;
    for raw in text.split('\n') {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(Span::styled(format!("{}{}", if first_line { first } else { rest }, raw), dim())));
            first_line = false;
            continue;
        }
        let st = if in_code {
            Style::default().fg(Color::Gray)
        } else if raw.starts_with('#') {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        let pieces: Vec<String> = if raw.is_empty() { vec![String::new()] } else { textwrap::wrap(raw, w).into_iter().map(|c| c.into_owned()).collect() };
        for p in pieces {
            let prefix = if first_line { first } else { rest };
            first_line = false;
            out.push(Line::from(vec![Span::styled(prefix.to_string(), dim()), Span::styled(p, st)]));
        }
    }
}

fn transcript(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for e in &app.entries {
        match e {
            Entry::User(t) => {
                out.push(Line::default());
                wrap_into(&mut out, t, width, "› ", "  ", Style::default().add_modifier(Modifier::BOLD));
            }
            Entry::Assistant(t) => {
                out.push(Line::default());
                wrap_into(&mut out, t.trim_end(), width, "  ", "  ", Style::default());
            }
            Entry::Tool { summary, state } => {
                let (mark, st) = match state {
                    ToolState::Running => (SPIN[app.spinner % SPIN.len()].to_string(), Style::default().fg(Color::Yellow)),
                    ToolState::Ok => ("●".into(), Style::default().fg(Color::Green)),
                    ToolState::Failed(_) => ("✗".into(), Style::default().fg(Color::Red)),
                };
                let s: String = summary.chars().take(width.saturating_sub(6)).collect();
                out.push(Line::from(vec![Span::raw("  "), Span::styled(mark, st), Span::raw(" "), Span::styled(s, Style::default().fg(Color::Gray))]));
                if let ToolState::Failed(msg) = state {
                    let m: String = msg.chars().take(width.saturating_sub(6)).collect();
                    out.push(Line::from(Span::styled(format!("    {m}"), Style::default().fg(Color::Red))));
                }
            }
            Entry::File { rel, added, removed, created } => {
                out.push(Line::from(vec![
                    Span::styled(format!("    {} ", if *created { "new " } else { "edit" }), dim()),
                    Span::raw(rel.clone()),
                    Span::styled(format!("  +{added}"), Style::default().fg(Color::Green)),
                    Span::styled(format!(" -{removed}"), Style::default().fg(Color::Red)),
                ]));
            }
            Entry::Notice(t) => {
                out.push(Line::default());
                wrap_into(&mut out, t, width, "  ", "  ", dim());
            }
            Entry::Error(t) => {
                out.push(Line::default());
                wrap_into(&mut out, t, width, "! ", "  ", Style::default().fg(Color::Red));
            }
        }
    }
    out
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let input_lines = app.input.split('\n').count().clamp(1, 8) as u16;
    let pending_h: u16 = match &app.pending {
        Some(Pending::Approve { .. }) => 3,
        Some(Pending::Ask { questions, answers, .. }) => {
            let opts = questions.get(answers.len()).map(|(_, o)| o.len()).unwrap_or(0) as u16;
            3 + opts.min(6)
        }
        None => 0,
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(pending_h), Constraint::Length(1), Constraint::Length(input_lines + 2)])
        .split(area);

    // Header.
    let project = app.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| app.root.display().to_string());
    let header = Line::from(vec![
        Span::styled(" blaze ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  {project}  ")),
        Span::styled(format!("{}  ", app.model), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{} · {}", app.mode, app.approval.label()), dim()),
    ]);
    f.render_widget(Paragraph::new(header), chunks[0]);

    // Transcript (scrolled from the bottom).
    let width = chunks[1].width as usize;
    let lines = transcript(app, width.saturating_sub(1));
    let h = chunks[1].height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    let scroll = app.scroll.min(max_scroll);
    let start = lines.len().saturating_sub(h + scroll);
    let end = (start + h).min(lines.len());
    f.render_widget(Paragraph::new(lines[start..end].to_vec()), chunks[1]);

    // Pending approval / question.
    match &app.pending {
        Some(Pending::Approve { summary, .. }) => {
            let p = Paragraph::new(vec![
                Line::from(Span::styled(format!("Allow: {summary}"), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled("[y] yes   [a] always for this tool   [n] no", dim())),
            ])
            .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::Yellow)));
            f.render_widget(p, chunks[2]);
        }
        Some(Pending::Ask { questions, answers, .. }) => {
            if let Some((q, opts)) = questions.get(answers.len()) {
                let mut l = vec![Line::from(Span::styled(q.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))];
                for (i, o) in opts.iter().take(6).enumerate() {
                    l.push(Line::from(Span::raw(format!("  {}. {o}", i + 1))));
                }
                l.push(Line::from(Span::styled("Type a number or your own answer, then Enter.", dim())));
                f.render_widget(Paragraph::new(l).block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::Yellow))), chunks[2]);
            }
        }
        None => {}
    }

    // Status line.
    let status = if app.is_running() {
        format!(" {} working · {}s · Esc to stop", SPIN[app.spinner % SPIN.len()], app.elapsed())
    } else if !app.status.is_empty() {
        format!(" {}", app.status)
    } else {
        " Enter send · Alt+Enter newline · Tab mode · Ctrl+L model · /help".to_string()
    };
    let k = |n: u64| if n >= 10_000 { format!("{}k", n / 1000) } else if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() };
    let tok = if app.tokens.0 + app.tokens.1 > 0 { format!("{} in · {} out ", k(app.tokens.0), k(app.tokens.1)) } else { String::new() };
    let sw = chunks[3].width as usize;
    let pad = sw.saturating_sub(status.width() + tok.width());
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(status, dim()), Span::raw(" ".repeat(pad)), Span::styled(tok, dim())])), chunks[3]);

    // Input.
    let block = Block::default().borders(Borders::ALL).border_style(dim());
    let inner = block.inner(chunks[4]);
    f.render_widget(Paragraph::new(app.input.clone()).block(block), chunks[4]);
    if app.picker.is_none() && !matches!(app.pending, Some(Pending::Approve { .. })) {
        let before = &app.input[..app.cursor];
        let row = before.matches('\n').count() as u16;
        let col = before.rsplit('\n').next().unwrap_or("").width() as u16;
        f.set_cursor_position((inner.x + col.min(inner.width.saturating_sub(1)), inner.y + row.min(inner.height.saturating_sub(1))));
    }

    // Model picker overlay.
    if let Some(p) = &app.picker {
        let w = area.width.saturating_sub(8).min(90);
        let hgt = area.height.saturating_sub(6).min(22);
        let rect = Rect { x: (area.width - w) / 2, y: (area.height - hgt) / 2, width: w, height: hgt };
        f.render_widget(Clear, rect);
        let items = app.picker_items();
        let list: Vec<ListItem> = items
            .iter()
            .map(|m| {
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{:<38}", m.id)),
                    Span::styled(format!(" {}", crate::session::price_text(m)), dim()),
                ]))
            })
            .collect();
        let title = format!(" Model: {}▏ ({} found, Enter pick, Esc close) ", p.filter, items.len());
        let mut state = ListState::default();
        state.select(if items.is_empty() { None } else { Some(p.sel.min(items.len() - 1)) });
        let widget = List::new(list).block(Block::default().borders(Borders::ALL).title(title)).highlight_style(Style::default().bg(Color::White).fg(Color::Black));
        f.render_stateful_widget(widget, rect, &mut state);
    }
}
