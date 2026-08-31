#!/usr/bin/env python3
"""Generate the website's block font from src/art.rs.

docs/index.html says its hero is "set in the same typeface the program actually
draws with". It was a hand copy of the letters: every digit and every punctuation
glyph was missing, and nothing could notice, because a website does not fail to
compile. The table is generated now, and CI runs this with --check so the claim
cannot come apart again.

    tools/gen-site-font.py            rewrite docs/index.html in place
    tools/gen-site-font.py --check    fail if it is not already up to date
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ART = ROOT / "src" / "art.rs"
SITE = ROOT / "docs" / "index.html"

BEGIN = "/* BEGIN GENERATED FONT -- tools/gen-site-font.py, from src/art.rs */"
END = "/* END GENERATED FONT */"

# 'A' => &[" ### ", "#   #", ...],   and   '\'' => &["#", ...],
ARM = re.compile(r"^\s*'((?:\\.)|[^'])'\s*=>\s*&\[(.+?)\],\s*$", re.M)
ROW = re.compile(r'"((?:[^"\\]|\\.)*)"')
SUPPORTED = re.compile(r'pub const SUPPORTED: &str = "((?:[^"\\]|\\.)*)";')


def unescape(rust: str) -> str:
    return rust.replace("\\'", "'").replace('\\"', '"').replace("\\\\", "\\")


def read_font() -> "list[tuple[str, list[str]]]":
    art = ART.read_text()
    m = SUPPORTED.search(art)
    if not m:
        sys.exit(f"{ART}: no `pub const SUPPORTED` to take the character order from")
    order = unescape(m.group(1))

    glyphs = {}
    for ch, rows in ARM.findall(art):
        ch = unescape(ch)
        glyphs[ch] = [unescape(r) for r in ROW.findall(rows)]

    out = []
    for ch in order:
        if ch not in glyphs:
            sys.exit(f"{ART}: SUPPORTED lists {ch!r} but there is no glyph arm for it")
        out.append((ch, glyphs[ch]))
    return out


def render(font) -> str:
    def key(ch: str) -> str:
        return ch if ch.isalnum() else '"%s"' % ch.replace('"', '\\"')

    lines = [BEGIN, "const GLYPHS = {"]
    for ch, rows in font:
        body = ",".join('"%s"' % r for r in rows)
        lines.append(f"  {key(ch)}:[{body}],")
    lines.append("};")
    lines.append(END)
    return "\n".join(lines)


def main() -> int:
    check = "--check" in sys.argv[1:]
    html = SITE.read_text()
    if BEGIN not in html or END not in html:
        sys.exit(f"{SITE}: missing the {BEGIN!r} / {END!r} markers")
    head, rest = html.split(BEGIN, 1)
    _, tail = rest.split(END, 1)
    updated = head + render(read_font()) + tail
    if updated == html:
        print("docs/index.html: font is up to date")
        return 0
    if check:
        print("FAIL: docs/index.html font is stale -- run tools/gen-site-font.py", file=sys.stderr)
        return 1
    SITE.write_text(updated)
    print("docs/index.html: font regenerated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
