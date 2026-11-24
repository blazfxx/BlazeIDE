//! QR codes for pairing a phone: an SVG for the IDE, block characters for the
//! terminal.

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};

pub fn svg(text: &str) -> String {
    match QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M) {
        Ok(code) => code.render::<svg::Color>().min_dimensions(220, 220).quiet_zone(true).dark_color(svg::Color("#000000")).light_color(svg::Color("#ffffff")).build(),
        Err(_) => String::new(),
    }
}

/// Two QR rows per text line using half blocks, with a quiet zone, readable
/// by a phone camera from most terminals.
pub fn terminal(text: &str) -> String {
    let Ok(code) = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L) else { return String::new() };
    let w = code.width();
    let colors = code.to_colors();
    let dark = |x: isize, y: isize| -> bool {
        if x < 0 || y < 0 || x >= w as isize || y >= w as isize {
            return false;
        }
        colors[y as usize * w + x as usize] == qrcode::Color::Dark
    };
    let q = 2isize;
    let mut out = String::new();
    let mut y = -q;
    while y < w as isize + q {
        for x in -q..w as isize + q {
            // Light background, dark modules: print inverted so it works on dark terminals.
            let (top, bottom) = (dark(x, y), dark(x, y + 1));
            out.push(match (top, bottom) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders() {
        let s = super::svg("https://example.com/#pair=abc");
        assert!(s.contains("<svg") && s.contains("</svg>"));
        let t = super::terminal("https://example.com/");
        assert!(t.lines().count() > 10);
    }
}
