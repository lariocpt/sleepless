#!/usr/bin/env python3
"""Gate docs/index.html before it is deployed.

The website is a release surface: pages.yml pushes whatever is in docs/ to the
public URL on any docs push, with nothing in front of it. This is what goes in
front of it.

Rule 1 is that the page makes no external request except to Google Fonts, which it
uses deliberately. That was previously not checked at all, and the obvious check --
a grep for `src="https:` -- is blind to single quotes, unquoted values,
protocol-relative `//` URLs, srcset, iframes, video, CSS url() and @import. This
walks every start tag with a real parser and looks at every attribute, and names the
line when it finds one.

A plain <a href> is exempt: a link the reader chooses to follow is not a request the
page makes.

    tools/check-site.py docs/index.html
"""

import re
import sys
from html.parser import HTMLParser
from pathlib import Path

ALLOWED_HOSTS = ("fonts.googleapis.com", "fonts.gstatic.com")
REMOTE = re.compile(r"^\s*(https?:)?//", re.I)
CSS_REMOTE = re.compile(r"url\(\s*['\"]?\s*(https?:)?//|@import", re.I)
# Elements that fetch by their nature, whatever their attributes are called.
EMBEDDERS = {"iframe", "object", "embed", "video", "audio", "source", "track", "frame"}
URL_ATTRS = {"src", "href", "srcset", "poster", "data", "action", "formaction", "ping"}


def allowed(url: str) -> bool:
    host = re.sub(r"^\s*(https?:)?//", "", url, flags=re.I).split("/")[0].split("?")[0]
    return host.lower() in ALLOWED_HOSTS


def srcset_urls(value: str) -> "list[str]":
    """The URLs in a srcset, without the `2x` / `640w` descriptors beside them."""
    out = []
    for candidate in value.split(","):
        parts = candidate.split()
        if parts:
            out.append(parts[0])
    return out


class Check(HTMLParser):
    def __init__(self, root: Path) -> None:
        super().__init__()
        self.root = root
        self.problems: "list[str]" = []
        self.in_style = False
        self.saw = {"title": False, "description": False}

    def fail(self, msg: str) -> None:
        line, col = self.getpos()
        self.problems.append(f"{line}:{col}: {msg}")

    def handle_starttag(self, tag, attrs):
        attrs = [(k.lower(), v or "") for k, v in attrs]
        d = dict(attrs)
        if tag == "style":
            self.in_style = True
        if tag == "meta" and d.get("name", "").lower() == "description":
            self.saw["description"] = bool(d.get("content", "").strip())
        if tag == "title":
            self.saw["title"] = True

        for key, value in attrs:
            if key == "style" and CSS_REMOTE.search(value):
                self.fail(f"<{tag} style=...> fetches something remote")
            if key not in URL_ATTRS:
                continue
            # A link is the one thing the reader initiates, not the page.
            if tag == "a" and key == "href" and tag not in EMBEDDERS:
                continue
            for url in srcset_urls(value) if key == "srcset" else [value]:
                if not url or not REMOTE.match(url):
                    self._local(tag, key, url)
                    continue
                if not allowed(url):
                    self.fail(f"<{tag} {key}> requests {url!r}, which is not a font host")

    def _local(self, tag, key, url):
        """A relative reference has to exist, or the deploy serves a broken page."""
        if not url or url.startswith(("#", "data:", "mailto:", "tel:")):
            return
        if tag == "a" and key == "href":
            return
        target = self.root / url.split("?")[0].split("#")[0]
        if not target.exists():
            self.fail(f"<{tag} {key}> points at {url!r}, which is not in docs/")

    def handle_endtag(self, tag):
        if tag == "style":
            self.in_style = False

    def handle_data(self, data):
        if self.in_style and CSS_REMOTE.search(data):
            self.fail("<style> fetches something remote")


def main(argv: "list[str]") -> int:
    if len(argv) != 2:
        sys.exit("usage: check-site.py docs/index.html")
    path = Path(argv[1]).resolve()
    check = Check(path.parent)
    check.feed(path.read_text())
    for name, ok in check.saw.items():
        if not ok:
            check.problems.append(f"the page has no <{name}>")
    if check.problems:
        print(f"FAIL: {path.name}", file=sys.stderr)
        for p in check.problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"{path.name}: self-contained apart from Google Fonts, and every local asset exists")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
