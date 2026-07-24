//! The splash: "I CAN'T GET NO SLEEP", rendered from a tiny built-in block font
//! so both sizes come from one glyph table (scale 2 = chunky, scale 1 = compact).

const ROWS: usize = 5;

const WORDS: [&str; 3] = ["I CAN'T", "GET NO", "SLEEP"];

fn glyph(c: char) -> &'static [&'static str; ROWS] {
    match c {
        'I' => &["#####", "  #  ", "  #  ", "  #  ", "#####"],
        'C' => &[" ####", "#    ", "#    ", "#    ", " ####"],
        'A' => &[" ### ", "#   #", "#####", "#   #", "#   #"],
        'N' => &["#   #", "##  #", "# # #", "#  ##", "#   #"],
        'T' => &["#####", "  #  ", "  #  ", "  #  ", "  #  "],
        'G' => &[" ####", "#    ", "# ###", "#   #", " ####"],
        'E' => &["#####", "#    ", "#### ", "#    ", "#####"],
        'O' => &[" ### ", "#   #", "#   #", "#   #", " ### "],
        'S' => &[" ####", "#    ", " ### ", "    #", "#### "],
        'L' => &["#    ", "#    ", "#    ", "#    ", "#####"],
        'P' => &["#### ", "#   #", "#### ", "#    ", "#    "],
        '\'' => &["#", "#", " ", " ", " "],
        _ => &["  ", "  ", "  ", "  ", "  "], // space and anything unknown
    }
}

fn render_word(word: &str, scale: usize) -> Vec<String> {
    let mut rows = vec![String::new(); ROWS];
    for (gi, ch) in word.chars().enumerate() {
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

fn render(scale: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (i, word) in WORDS.iter().enumerate() {
        if i > 0 {
            out.push(String::new());
        }
        out.extend(render_word(word, scale));
    }
    out
}

/// Rows below the art (banner, status, power, footer, spacing).
const CHROME_ROWS: u16 = 6;

/// Best art for the terminal size; None means fall back to a plain one-liner.
pub fn pick(width: u16, height: u16) -> Option<Vec<String>> {
    for scale in [2usize, 1] {
        let lines = render(scale);
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
        let h = lines.len() as u16;
        if width >= w + 2 && height >= h + CHROME_ROWS {
            return Some(lines);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_art_fits_80_columns() {
        let lines = pick(80, 30).expect("art should fit a standard terminal");
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(w <= 78, "widest line is {w}");
        assert_eq!(lines.len(), 17); // 3 words x 5 rows + 2 separators
    }

    #[test]
    fn compact_art_fits_narrow_terminals() {
        let lines = pick(45, 30).expect("compact art should fit 45 cols");
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(w <= 43, "widest line is {w}");
    }

    #[test]
    fn tiny_terminal_gets_plain_text() {
        assert!(pick(30, 10).is_none());
    }

    #[test]
    fn every_word_renders_consistent_rows() {
        for word in WORDS {
            let rows = render_word(word, 1);
            assert_eq!(rows.len(), ROWS);
            let widths: Vec<_> = rows.iter().map(|r| r.chars().count()).collect();
            assert!(
                widths.iter().all(|w| *w == widths[0]),
                "{word}: ragged rows {widths:?}"
            );
        }
    }
}
