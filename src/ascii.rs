//! Plain-ASCII output for `--smoke`.
//!
//! The headless status line is what CI greps and what a script parses, and on
//! Windows it goes through a console code page. Most of it is ASCII by
//! construction, but three parts are not ours to control: D-Bus error strings,
//! config file paths, and the `--why` text. CI asserting "the output was ASCII"
//! only ever checked the one path it happened to exercise; squashing at the print
//! site means a non-ASCII byte cannot reach a grep on any path, including the
//! error paths that are the likeliest source of one.

/// Force a string onto one line of printable ASCII.
pub fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' | '\t' | '\n' | '\r' => out.push(' '),
            c if c.is_ascii_graphic() => out.push(c),
            // The handful of non-ASCII characters this program emits itself get
            // spelled out rather than blanked, so the line stays readable.
            '\u{2014}' | '\u{2013}' | '\u{2011}' => out.push('-'), // em/en/non-breaking dash
            '\u{00b7}' => out.push('.'),                           // middle dot
            '\u{2713}' => out.push_str("yes"),                     // check
            '\u{2717}' => out.push_str("no"),                      // ballot x
            '\u{25cf}' | '\u{25cc}' => out.push('*'),              // filled/dotted circle
            _ => out.push('?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_ascii_through_unchanged() {
        let s = "sleepless - AWAKE (always) | AC - battery 99% (Charging)";
        assert_eq!(squash(s), s);
    }

    #[test]
    fn spells_out_what_the_program_itself_emits() {
        assert_eq!(squash("AC \u{2014} battery"), "AC - battery");
        assert_eq!(
            squash("idle \u{2713} \u{00b7} sleep \u{2717}"),
            "idle yes . sleep no"
        );
    }

    #[test]
    fn replaces_anything_else_and_never_breaks_the_line() {
        // A D-Bus error, a config path or a --why string can contain anything.
        assert_eq!(squash("caf\u{e9} \u{1f50b}"), "caf? ?");
        assert_eq!(squash("first\nsecond\r\tthird"), "first second  third");
        // Whatever goes in, exactly one printable-ASCII line comes out.
        for s in ["\u{0}\u{7}", "\u{4e2d}\u{6587}", "\u{2028}"] {
            let got = squash(s);
            assert!(
                got.chars().all(|c| c == ' ' || c.is_ascii_graphic()),
                "{s:?} -> {got:?}"
            );
        }
    }
}
