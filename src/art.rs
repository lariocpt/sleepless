//! Splash screens: a tiny built-in block font (5 rows, A-Z 0-9 and a little
//! punctuation) for `text` splashes, plus verbatim-art support. Rendering
//! picks the largest scale that fits the terminal and falls back to a plain
//! one-liner when nothing fits.

use ratatui::style::Color;

pub struct Splash {
    pub name: String,
    pub source: SplashSource,
    /// Art color while holding locks.
    pub color: Color,
    /// Brighter pulse frame color.
    pub pulse_color: Color,
    /// Art color while paused.
    pub paused_color: Color,
}

pub enum SplashSource {
    /// Lines rendered with the block font.
    Text(Vec<String>),
    /// Verbatim ASCII art lines.
    Art(Vec<String>),
}

impl Splash {
    /// Render for a terminal size; None = use `fallback_line` instead.
    pub fn render(&self, width: u16, height: u16) -> Option<Vec<String>> {
        match &self.source {
            SplashSource::Text(lines) => pick_text(lines, width, height),
            SplashSource::Art(lines) => {
                let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
                fits(width, height, w, lines.len()).then(|| lines.clone())
            }
        }
    }

    /// One-line fallback for terminals too small for the art.
    pub fn fallback_line(&self) -> String {
        match &self.source {
            SplashSource::Text(lines) => lines.join(" "),
            SplashSource::Art(_) => self.name.to_uppercase(),
        }
    }
}

const ROWS: usize = 5;

/// Rows below the art (banner, status, power, footer, spacing).
const CHROME_ROWS: u16 = 6;

/// Does `w` x `h` cells of art fit a `width` x `height` terminal with room left for
/// the chrome?
///
/// Done by widening the terminal rather than narrowing the art. `art_file` is an
/// arbitrary file, so `w as u16 + 2` was two bugs at once: it panics in a debug
/// build past 65533 columns, and in release it wraps -- a 70 000-column line came
/// out as 4464 and "fitted" an 80-column terminal.
fn fits(width: u16, height: u16, w: usize, h: usize) -> bool {
    width as usize >= w.saturating_add(2) && height as usize >= h + CHROME_ROWS as usize
}

/// Every character the block font has a real glyph for. The README documents this
/// set and `tests/docs.rs` asserts the two agree, because the set is invisible from
/// the outside: an unsupported character renders as a blank rather than failing, so
/// documenting one that does not exist looks exactly like documenting one that does.
pub const SUPPORTED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'-!?.";

/// What an unsupported character renders as: right for a space, harmless elsewhere.
const BLANK: &[&str; ROWS] = &["  ", "  ", "  ", "  ", "  "];

/// Does the block font have a glyph for `c`?
pub fn has_glyph(c: char) -> bool {
    glyph(c) != BLANK
}

pub fn glyph(c: char) -> &'static [&'static str; ROWS] {
    match c.to_ascii_uppercase() {
        'A' => &[" ### ", "#   #", "#####", "#   #", "#   #"],
        'B' => &["#### ", "#   #", "#### ", "#   #", "#### "],
        'C' => &[" ####", "#    ", "#    ", "#    ", " ####"],
        'D' => &["#### ", "#   #", "#   #", "#   #", "#### "],
        'E' => &["#####", "#    ", "#### ", "#    ", "#####"],
        'F' => &["#####", "#    ", "#### ", "#    ", "#    "],
        'G' => &[" ####", "#    ", "# ###", "#   #", " ####"],
        'H' => &["#   #", "#   #", "#####", "#   #", "#   #"],
        'I' => &["#####", "  #  ", "  #  ", "  #  ", "#####"],
        'J' => &["    #", "    #", "    #", "#   #", " ### "],
        'K' => &["#   #", "#  # ", "###  ", "#  # ", "#   #"],
        'L' => &["#    ", "#    ", "#    ", "#    ", "#####"],
        'M' => &["#   #", "## ##", "# # #", "#   #", "#   #"],
        'N' => &["#   #", "##  #", "# # #", "#  ##", "#   #"],
        'O' => &[" ### ", "#   #", "#   #", "#   #", " ### "],
        'P' => &["#### ", "#   #", "#### ", "#    ", "#    "],
        'Q' => &[" ### ", "#   #", "#   #", "#  # ", " ## #"],
        'R' => &["#### ", "#   #", "#### ", "#  # ", "#   #"],
        'S' => &[" ####", "#    ", " ### ", "    #", "#### "],
        'T' => &["#####", "  #  ", "  #  ", "  #  ", "  #  "],
        'U' => &["#   #", "#   #", "#   #", "#   #", " ### "],
        'V' => &["#   #", "#   #", "#   #", " # # ", "  #  "],
        'W' => &["#   #", "#   #", "# # #", "## ##", "#   #"],
        'X' => &["#   #", " # # ", "  #  ", " # # ", "#   #"],
        'Y' => &["#   #", " # # ", "  #  ", "  #  ", "  #  "],
        'Z' => &["#####", "   # ", "  #  ", " #   ", "#####"],
        '0' => &[" ### ", "#  ##", "# # #", "##  #", " ### "],
        '1' => &["  #  ", " ##  ", "  #  ", "  #  ", "#####"],
        '2' => &[" ### ", "#   #", "  ## ", " #   ", "#####"],
        '3' => &["#### ", "    #", " ### ", "    #", "#### "],
        '4' => &["#  # ", "#  # ", "#####", "   # ", "   # "],
        '5' => &["#####", "#    ", "#### ", "    #", "#### "],
        '6' => &[" ### ", "#    ", "#### ", "#   #", " ### "],
        '7' => &["#####", "    #", "   # ", "  #  ", "  #  "],
        '8' => &[" ### ", "#   #", " ### ", "#   #", " ### "],
        '9' => &[" ### ", "#   #", " ####", "    #", " ### "],
        '\'' => &["#", "#", " ", " ", " "],
        '-' => &["    ", "    ", "####", "    ", "    "],
        '!' => &["#", "#", "#", " ", "#"],
        '?' => &[" ### ", "#   #", "  ## ", "     ", "  #  "],
        '.' => &[" ", " ", " ", " ", "#"],
        _ => BLANK, // space and anything unknown
    }
}

fn render_line(text: &str, scale: usize) -> Vec<String> {
    let mut rows = vec![String::new(); ROWS];
    for (gi, ch) in text.chars().enumerate() {
        let g = glyph(ch);
        for (r, row) in rows.iter_mut().enumerate() {
            if gi > 0 {
                row.push_str(&" ".repeat(scale));
            }
            for cell in g[r].chars() {
                let block = if cell == '#' { "█" } else { " " };
                for _ in 0..scale {
                    row.push_str(block);
                }
            }
        }
    }
    rows
}

fn render_lines(lines: &[String], scale: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push(String::new());
        }
        out.extend(render_line(line, scale));
    }
    out
}

fn pick_text(lines: &[String], width: u16, height: u16) -> Option<Vec<String>> {
    for scale in [2usize, 1] {
        let rendered = render_lines(lines, scale);
        let w = rendered
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        if fits(width, height, w, rendered.len()) {
            return Some(rendered);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_words() -> Vec<String> {
        vec!["I CAN'T".into(), "GET NO".into(), "SLEEP".into()]
    }

    #[test]
    fn wide_art_fits_80_columns() {
        let lines = pick_text(&default_words(), 80, 30).expect("should fit a standard terminal");
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(w <= 78, "widest line is {w}");
        assert_eq!(lines.len(), 17); // 3 words x 5 rows + 2 separators
    }

    #[test]
    fn compact_art_fits_narrow_terminals() {
        let lines = pick_text(&default_words(), 45, 30).expect("compact should fit 45 cols");
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(w <= 43, "widest line is {w}");
    }

    #[test]
    fn tiny_terminal_gets_plain_text() {
        assert!(pick_text(&default_words(), 30, 10).is_none());
    }

    #[test]
    fn the_supported_set_is_exactly_what_has_glyphs() {
        // SUPPORTED is a second spelling of the match arms below it, and the README
        // is a third. This ties the first two together; tests/docs.rs ties in the
        // README.
        for c in SUPPORTED.chars() {
            assert!(has_glyph(c), "SUPPORTED lists {c:?}, which has no glyph");
        }
        for c in " ~@#$%^&*()_+=[]{}|;:,<>/\\\"".chars() {
            assert!(!has_glyph(c), "{c:?} is not in SUPPORTED but has a glyph");
        }
        assert_eq!(
            SUPPORTED.chars().count(),
            SUPPORTED
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "SUPPORTED has a duplicate"
        );
    }

    #[test]
    fn all_supported_glyphs_are_consistent() {
        for c in ('A'..='Z').chain('0'..='9').chain("'-!?. ~".chars()) {
            let g = glyph(c);
            assert_eq!(g.len(), ROWS);
            let w = g[0].chars().count();
            assert!(
                g.iter().all(|r| r.chars().count() == w),
                "glyph {c:?} has ragged rows"
            );
            assert!(
                g.iter().all(|r| r.chars().all(|ch| ch == '#' || ch == ' ')),
                "glyph {c:?} has invalid characters"
            );
        }
    }

    #[test]
    fn arbitrary_text_renders() {
        let rows = render_line("HELLO WORLD 123!", 1);
        assert_eq!(rows.len(), ROWS);
        let widths: Vec<_> = rows.iter().map(|r| r.chars().count()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "ragged: {widths:?}");
    }

    #[test]
    fn oversized_art_never_fits_and_never_panics() {
        // An art_file is whatever the user points at. The fit check used to cast the
        // widest line to u16 and add 2: past 65533 columns that panics in debug, and
        // in release it wraps, so a 70 000-column line "fitted" an 80-column terminal.
        let huge = Splash {
            name: "huge".into(),
            source: SplashSource::Art(vec!["#".repeat(70_000)]),
            color: Color::Green,
            pulse_color: Color::LightGreen,
            paused_color: Color::DarkGray,
        };
        assert!(
            huge.render(80, 40).is_none(),
            "a 70k-column line cannot fit 80"
        );
        assert!(huge.render(u16::MAX, u16::MAX).is_none());

        let tall = Splash {
            name: "tall".into(),
            source: SplashSource::Art(vec!["#".into(); 70_000]),
            color: Color::Green,
            pulse_color: Color::LightGreen,
            paused_color: Color::DarkGray,
        };
        assert!(tall.render(80, 40).is_none(), "70k rows cannot fit 40");
        assert!(tall.render(u16::MAX, u16::MAX).is_none());

        // And the exact boundary is still right for ordinary art.
        assert!(fits(10, 10, 8, 4), "8+2 columns in 10, 4+6 rows in 10");
        assert!(!fits(9, 10, 8, 4), "one column short");
        assert!(!fits(10, 9, 8, 4), "one row short");
    }

    #[test]
    fn raw_art_fit_and_fallback() {
        let splash = Splash {
            name: "cat".into(),
            source: SplashSource::Art(vec![" /\\_/\\".into(), "( o.o )".into()]),
            color: Color::Green,
            pulse_color: Color::LightGreen,
            paused_color: Color::DarkGray,
        };
        assert!(splash.render(40, 20).is_some());
        assert!(splash.render(6, 20).is_none(), "too narrow for the art");
        assert_eq!(splash.fallback_line(), "CAT");
    }
}
