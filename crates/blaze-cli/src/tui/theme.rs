//! Light and dark palettes, and finding out which one the terminal needs.
//!
//! `cliTheme` setting: "auto" (default), "dark" or "light" (`/theme`).
//! Auto asks the terminal for its background colour (OSC 11, answered by
//! Windows Terminal, iTerm2, Terminal.app, kitty, WezTerm, GNOME...), then
//! falls back to COLORFGBG, then to dark.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ratatui::style::Color;

static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

pub struct Pal {
    pub accent: Color,
    /// Background of your messages, the prompt, the title and panels.
    pub shade: Color,
    /// Selected rows and code blocks.
    pub shade2: Color,
    /// Main text on a shaded block.
    pub text: Color,
    /// Secondary text.
    pub soft: Color,
    /// Hints.
    pub dim: Color,
    pub rule: Color,
    pub code: Color,
    pub warn: Color,
}

const DARK: Pal = Pal {
    accent: Color::Indexed(208),
    shade: Color::Indexed(235),
    shade2: Color::Indexed(237),
    text: Color::Indexed(255),
    soft: Color::Indexed(250),
    dim: Color::Indexed(243),
    rule: Color::Indexed(240),
    code: Color::Indexed(180),
    warn: Color::Indexed(220),
};

const LIGHT_PAL: Pal = Pal {
    accent: Color::Indexed(166),
    shade: Color::Indexed(255),
    shade2: Color::Indexed(253),
    text: Color::Indexed(234),
    soft: Color::Indexed(239),
    dim: Color::Indexed(245),
    rule: Color::Indexed(250),
    code: Color::Indexed(94),
    warn: Color::Indexed(130),
};

pub fn pal() -> &'static Pal {
    if is_light() {
        &LIGHT_PAL
    } else {
        &DARK
    }
}

/// Relative luminance of an OSC 11 answer such as
/// `\x1b]11;rgb:ffff/ffff/ffff\x1b\\` (1-4 hex digits per channel).
pub fn parse_osc11(reply: &str) -> Option<f32> {
    let i = reply.find("rgb:")?;
    let rest = &reply[i + 4..];
    let mut ch = [0f32; 3];
    for (n, part) in rest.split('/').take(3).enumerate() {
        let hex: String = part.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if hex.is_empty() {
            return None;
        }
        let max = (16u32.pow(hex.len() as u32) - 1) as f32;
        ch[n] = u32::from_str_radix(&hex, 16).ok()? as f32 / max;
    }
    Some(0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2])
}

/// `COLORFGBG=15;0` (fg;bg): bg 7 or 9-15 is light.
pub fn colorfgbg_light(v: &str) -> Option<bool> {
    let bg: u32 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(bg == 7 || (9..=15).contains(&bg))
}

/// Ask the terminal. Raw mode must be on and nothing else reading input yet.
pub fn detect() -> Option<bool> {
    if let Some(l) = query_osc11().and_then(|r| parse_osc11(&r)) {
        return Some(l > 0.5);
    }
    std::env::var("COLORFGBG").ok().and_then(|v| colorfgbg_light(&v))
}

#[cfg(unix)]
fn query_osc11() -> Option<String> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
    tty.write_all(b"\x1b]11;?\x1b\\").ok()?;
    tty.flush().ok()?;
    let fd = tty.as_raw_fd();
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut buf = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for an fd we own for the duration of the call.
        let r = unsafe { libc::poll(&mut p, 1, left.as_millis() as libc::c_int) };
        if r <= 0 {
            break;
        }
        let mut chunk = [0u8; 64];
        let n = tty.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.ends_with(b"\x07") || buf.ends_with(b"\x1b\\") {
            break;
        }
    }
    (!buf.is_empty()).then(|| String::from_utf8_lossy(&buf).into_owned())
}

/// Windows: the answer arrives as key events (ESC ] 1 1 ; r g b : ...).
#[cfg(windows)]
fn query_osc11() -> Option<String> {
    use crossterm::event::{self, Event, KeyCode};
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]11;?\x1b\\").ok()?;
    out.flush().ok()?;
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut s = String::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if !event::poll(left).ok()? {
            break;
        }
        match event::read().ok()? {
            Event::Key(k) => match k.code {
                KeyCode::Char(c) => s.push(c),
                KeyCode::Esc => s.push('\x1b'),
                _ => {}
            },
            _ => {}
        }
        if s.contains("rgb:") && (s.ends_with('\\') || s.ends_with('\x07') || s.matches('/').count() >= 2 && s.len() > 20) {
            break;
        }
    }
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_backgrounds() {
        assert!(parse_osc11("\x1b]11;rgb:ffff/ffff/ffff\x1b\\").unwrap() > 0.9);
        assert!(parse_osc11("\x1b]11;rgb:0c0c/0c0c/0c0c\x07").unwrap() < 0.1);
        assert!(parse_osc11("\x1b]11;rgb:fa/f8/f0\x1b\\").unwrap() > 0.9);
        assert!(parse_osc11("nonsense").is_none());
        assert_eq!(colorfgbg_light("0;15"), Some(true));
        assert_eq!(colorfgbg_light("15;0"), Some(false));
        assert_eq!(colorfgbg_light("x"), None);
    }
}
