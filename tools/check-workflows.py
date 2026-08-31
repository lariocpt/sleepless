#!/usr/bin/env python3
"""Assert the properties the workflows are supposed to have.

A generic linter checks syntax. These are the specific things that were wrong, and
the point of writing them down is that none of them could fail before:

  * a release published before anything was built or tested;
  * a re-run that deleted the live release and replaced its assets, invalidating the
    checksums the Homebrew formula and the AUR PKGBUILD pin;
  * `cancel-in-progress` on main, so two quick merges left the first commit with no
    completed gate at all;
  * shell steps with no pipefail, where a crashed binary piped into `tee` is a pass;
  * the website deploying with nothing in front of it;
  * permissions granted and never used.

    tools/check-workflows.py
"""

import re
import shutil
import sys
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WF = ROOT / ".github" / "workflows"

problems: "list[str]" = []


def check(ok: bool, msg: str) -> None:
    if not ok:
        problems.append(msg)


def read(name: str) -> str:
    p = WF / name
    if not p.is_file():
        problems.append(f"{name} is missing")
        return ""
    return p.read_text()


def run_blocks(text: str) -> "list[tuple[int, str]]":
    """Every `run: |` block, with the line it starts on."""
    out, lines = [], text.splitlines()
    for i, line in enumerate(lines):
        m = re.match(r"^(\s*)-?\s*run:\s*\|", line)
        if not m:
            continue
        indent = len(m.group(1)) + 2
        body = []
        for nxt in lines[i + 1 :]:
            if nxt.strip() and len(nxt) - len(nxt.lstrip()) < indent:
                break
            body.append(nxt)
        out.append((i + 1, "\n".join(body)))
    return out


def step_shells(text: str) -> "list[str]":
    return re.findall(r"^\s*shell:\s*(\S+)", text, re.M)


ci = read("ci.yml")
release = read("release.yml")
pages = read("pages.yml")

# --- CI ---------------------------------------------------------------------
check(
    "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in ci,
    "ci.yml: cancel-in-progress must be pull-request-only, or two quick merges leave "
    "the first commit on main with no completed gate",
)
check("permissions:" in ci, "ci.yml: declare permissions explicitly")
for job in ("msrv", "test"):
    check(f"  {job}:" in ci, f"ci.yml: the {job} job disappeared")
check(
    "windows-latest" in ci.split("msrv:")[1].split("test:")[0],
    "ci.yml: the MSRV job must include Windows -- the 1.89 floor comes from a "
    "dependency that only macOS and Windows pull in",
)
test_job = ci.split("\n  test:\n")[-1]
for needed in ("ubuntu-24.04-arm", "windows-11-arm", "test/smoke.sh", "test/smoke.ps1"):
    check(
        needed in test_job,
        f"ci.yml: the test job must cover {needed} -- arm64 used to get `cargo test` "
        f"and nothing else, while the website claimed CI verified the lock on arm64",
    )
check(
    "workflow_call:" in ci,
    "ci.yml: must be callable, so release.yml runs this gate rather than a copy of it",
)

# --- release ----------------------------------------------------------------
# Matched on `uses:` lines only: the file explains in a comment why this action is
# gone, and a naive substring search would flag its own explanation.
check(
    re.search(r"^\s*-?\s*uses:.*create-gh-release-action", release, re.M) is None,
    "release.yml: create-gh-release-action deletes an existing release before "
    "recreating it, so a re-run takes the live one away and gives back different bytes",
)
check("--draft" in release, "release.yml: the release must be created as a draft")
draft_at = release.find("--draft")
flip_at = release.find("--draft=false")
check(
    flip_at > draft_at > 0,
    "release.yml: the draft must be flipped live after the assets are verified, not before",
)
check(
    "gh release download" in release,
    "release.yml: it must download its own published assets back and check them; "
    "verifying the local build proves nothing about what was uploaded",
)
check("sha256sum -c" in release, "release.yml: verify the checksums it publishes")
for needed, why in [
    ("Cargo.toml version", "assert the tag matches the version the binary reports"),
    ("CHANGELOG.md", "refuse to publish a tag with no release notes"),
    ("tools/package.py", "build archives that reproduce"),
    ("attest-build-provenance", "use the attestations permission it asks for"),
    ("--latest", "never demote the current release by re-publishing an older tag"),
]:
    check(needed in release, f"release.yml: {why} ({needed!r} not found)")
check(
    "concurrency:" in release,
    "release.yml: two tag pushes must not race each other",
)
check(
    "if: failure()" in release,
    "release.yml: a failed run must put the release back into draft so `latest` "
    "falls back to the previous good one",
)
for perm in ("id-token: write", "attestations: write"):
    if perm in release:
        check(
            "attest-build-provenance" in release,
            f"release.yml: grants {perm} and never uses it",
        )

# --- pages ------------------------------------------------------------------
gate_before_deploy = pages.find("check-site.py") < pages.find("upload-pages-artifact")
check(
    "check-site.py" in pages and gate_before_deploy,
    "pages.yml: the site must be gated before it is uploaded -- it deploys on any "
    "docs push, with nothing else in front of it",
)

# --- every workflow ---------------------------------------------------------
for name, text in [("ci.yml", ci), ("release.yml", release), ("pages.yml", pages)]:
    if not text:
        continue
    for line, body in run_blocks(text):
        if "pwsh" in text[max(0, text.find(body) - 400) : text.find(body)]:
            continue
        if "set -euo pipefail" not in body and "$ErrorActionPreference" not in body:
            problems.append(
                f"{name}:{line}: a run block without `set -euo pipefail` -- without "
                f"pipefail a crashed command piped into `tee` is a pass"
            )
    for sh in step_shells(text):
        check(
            sh in ("bash", "pwsh"),
            f"{name}: unexpected shell {sh!r}; the default `bash -e` has no pipefail",
        )

# --- no expressions spliced into shell --------------------------------------
# A `${{ }}` inside a `run:` block is substituted textually before any shell parses
# it, so it is string concatenation into a command, not a variable. Values belong in
# `env:` and scripts should read "$NAME".
for name, text in [("ci.yml", ci), ("release.yml", release), ("pages.yml", pages)]:
    for line, body in run_blocks(text):
        if "${{" in body:
            problems.append(
                f"{name}:{line}: a GitHub expression is spliced into a shell script; "
                f"pass it through `env:` and read it as a variable"
            )

# --- shellcheck the embedded scripts ---------------------------------------
# The bugs this file exists for lived in `run:` blocks, which no linter sees unless
# something digs them out. Skipped with a note where shellcheck is not installed:
# a missing tool is not a broken workflow.
if shutil.which("shellcheck"):
    import subprocess
    import tempfile

    for name, text in [("ci.yml", ci), ("release.yml", release), ("pages.yml", pages)]:
        for line, body in run_blocks(text):
            if "$ErrorActionPreference" in body or "pwsh" in body:
                continue
            dedented = textwrap.dedent(body)
            with tempfile.NamedTemporaryFile("w", suffix=".sh", delete=False) as fh:
                fh.write("#!/usr/bin/env bash\n" + dedented)
                tmp = fh.name
            # GitHub expressions are substituted before the shell ever sees them, so
            # they are opaque to shellcheck; SC2154 would flag every one of them.
            r = subprocess.run(
                ["shellcheck", "--shell=bash", "--severity=warning", "--exclude=SC2154", tmp],
                capture_output=True,
                text=True,
            )
            if r.returncode != 0:
                out = r.stdout.replace(tmp, f"{name}:{line}")
                problems.append(f"{name}:{line}: shellcheck\n{out}")
            Path(tmp).unlink()
else:
    print("note: shellcheck is not installed; embedded run blocks were not linted")

if problems:
    print("FAIL: workflow invariants", file=sys.stderr)
    for p in problems:
        print(f"  {p}", file=sys.stderr)
    sys.exit(1)
print("workflows: every invariant holds")
