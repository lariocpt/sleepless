#!/usr/bin/env bash
# Move the packaging channels to a released version, from one command.
#
# The Homebrew formula, the AUR PKGBUILD and its .SRCINFO between them hard-code the
# version and four sha256 values in eleven places, with nothing to move them and
# nothing to check they agree with the release. Doing it by hand is how a formula
# ends up pinning a checksum for an artifact that has since been rebuilt.
#
# Run it after the GitHub release is live:
#
#     tools/bump-channels.sh 0.1.1              # rewrite the sibling repos
#     tools/bump-channels.sh 0.1.1 --check      # only verify they already agree
#
# It reads the checksums from the release's own .sha256 assets rather than
# re-hashing a local build, so what gets pinned is what is actually published.
set -euo pipefail

VERSION=${1:-}
MODE=${2:-write}
[ -n "$VERSION" ] || { echo "usage: $0 <version> [--check]" >&2; exit 2; }
TAG="v$VERSION"
REPO=${GH_REPO:-lariocpt/sleepless}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TAP=${TAP_DIR:-$ROOT/../homebrew-sleepless}
AUR=${AUR_DIR:-$ROOT/../aur-sleepless-bin}

# A channel that is not here cannot be a gate on the channels that are.
#
# The AUR package was written but never registered: aur.archlinux.org has no
# sleepless-bin and there is no aur-sleepless-bin repository, so no checkout of it
# exists anywhere. Requiring one made this tool refuse to move the Homebrew formula —
# the one packaging channel that is actually advertised — and that is the same mistake
# check-channels.sh already corrected for the LAN plane, whose address it stopped
# insisting on. A missing checkout is therefore a skipped channel, not a failure.
#
# Naming AUR_DIR is different. Setting it is an assertion that the channel exists, so
# when it does not, that is a real mistake and stays a hard failure.
AUR_WAS_SET=${AUR_DIR:-}

fail() { echo "FAIL: $*" >&2; exit 1; }

want=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$ROOT/Cargo.toml" | head -1)
[ "$want" = "$VERSION" ] || fail "Cargo.toml says $want, not $VERSION"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# One target per channel slot. The two musl builds are what both the tap's Linux
# arms and the AUR package serve, so they must be the same string in both files.
targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-unknown-linux-musl
  x86_64-unknown-linux-musl
)
declare -A SUM
for t in "${targets[@]}"; do
  # The checksum asset is the archive's own name plus .sha256, which is what
  # `sha256sum -c` wants to read out of it. Getting this wrong 404s silently in a
  # loop that otherwise looks like it worked.
  name="sleepless-$t-$TAG.tar.gz.sha256"
  gh release download "$TAG" --repo "$REPO" --dir "$work" --pattern "$name" \
    || fail "$TAG has no $name -- is the release published?"
  SUM[$t]=$(cut -d' ' -f1 "$work/$name")
  [ ${#SUM[$t]} -eq 64 ] || fail "$name does not contain a sha256"
done

# ---------------------------------------------------------------- Homebrew tap
formula="$TAP/Formula/sleepless.rb"
[ -f "$formula" ] || fail "no formula at $formula (set TAP_DIR)"
new_formula=$(mktemp)
python3 - "$formula" "$VERSION" \
  "${SUM[aarch64-apple-darwin]}" "${SUM[x86_64-apple-darwin]}" \
  "${SUM[aarch64-unknown-linux-musl]}" "${SUM[x86_64-unknown-linux-musl]}" \
  > "$new_formula" <<'PY'
import re, sys
src, version, *sums = sys.argv[1], sys.argv[2], *sys.argv[3:]
text = open(src).read()
text = re.sub(r'(?m)^(\s*version\s+)"[^"]*"', lambda m: f'{m.group(1)}"{version}"', text)
text = re.sub(r'/download/v[^/]+/sleepless-([^"]+?)-v[^"]+\.tar\.gz',
              lambda m: f'/download/v{version}/sleepless-{m.group(1)}-v{version}.tar.gz', text)
# The sha256 lines are in the same order as the url lines above them.
it = iter(sums)
text = re.sub(r'(?m)^(\s*sha256\s+)"[0-9a-f]{64}"',
              lambda m: f'{m.group(1)}"{next(it)}"', text)
sys.stdout.write(text)
PY

# ------------------------------------------------------------------------- AUR
pkgbuild="$AUR/PKGBUILD"
have_aur=1
if [ ! -f "$pkgbuild" ]; then
  [ -n "$AUR_WAS_SET" ] && fail "no PKGBUILD at $pkgbuild (set AUR_DIR)"
  have_aur=0
  echo "  SKIP: no AUR checkout at $AUR — the package is not registered, so this" >&2
  echo "        channel is not moved. Set AUR_DIR once it exists to move it here." >&2
fi
new_pkgbuild=$(mktemp)
if [ "$have_aur" = 1 ]; then
  sed -e "s/^pkgver=.*/pkgver=$VERSION/" \
      -e "s/^pkgrel=.*/pkgrel=1/" \
      -e "s/^sha256sums_x86_64=('.*')/sha256sums_x86_64=('${SUM[x86_64-unknown-linux-musl]}')/" \
      -e "s/^sha256sums_aarch64=('.*')/sha256sums_aarch64=('${SUM[aarch64-unknown-linux-musl]}')/" \
      "$pkgbuild" > "$new_pkgbuild"
fi

# --------------------------------------------------------------------- apply
diffs=0
apply() {  # <new> <dest>
  if cmp -s "$1" "$2"; then
    echo "  up to date: $2"
    return
  fi
  diffs=1
  if [ "$MODE" = "--check" ]; then
    echo "  STALE: $2"
    diff -u "$2" "$1" || true
  else
    cp "$1" "$2"
    echo "  updated: $2"
  fi
}
echo "sleepless $VERSION -> packaging channels"
apply "$new_formula" "$formula"
[ "$have_aur" = 1 ] && apply "$new_pkgbuild" "$pkgbuild"

# .SRCINFO is generated, never edited: hand-editing it is how it drifts from the
# PKGBUILD it is supposed to describe.
if [ "$have_aur" = 1 ] && [ "$MODE" != "--check" ]; then
  if command -v makepkg >/dev/null 2>&1; then
    ( cd "$AUR" && makepkg --printsrcinfo > .SRCINFO )
    echo "  regenerated: $AUR/.SRCINFO"
  else
    echo "  SKIP: makepkg is not installed; regenerate $AUR/.SRCINFO on an Arch host" >&2
  fi
fi

# ------------------------------------------------------------------- verify
echo "checking every channel serves the same bytes"
for t in "${targets[@]}"; do
  grep -q "${SUM[$t]}" "$formula" || fail "the formula does not pin $t (${SUM[$t]})"
done
for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  if [ "$have_aur" = 1 ]; then
    grep -q "${SUM[$t]}" "$pkgbuild" || fail "the PKGBUILD does not pin $t"
    if [ -f "$AUR/.SRCINFO" ] && [ "$MODE" != "--check" ]; then
      grep -q "${SUM[$t]}" "$AUR/.SRCINFO" || fail ".SRCINFO does not pin $t"
    fi
  fi
done
grep -q "\"$VERSION\"" "$formula" || fail "the formula is not on $VERSION"
if [ "$have_aur" = 1 ]; then
  grep -q "^pkgver=$VERSION\$" "$pkgbuild" || fail "the PKGBUILD is not on $VERSION"
  echo "ok: formula, PKGBUILD and .SRCINFO all serve $VERSION and the released checksums"
else
  echo "ok: the formula serves $VERSION and the released checksums; the AUR channel is"
  echo "    not registered and was skipped rather than gated on"
fi

[ "$MODE" = "--check" ] && exit "$diffs"
exit 0
