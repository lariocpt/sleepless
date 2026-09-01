#!/usr/bin/env bash
# Does every install method the README advertises actually serve this version?
#
# Written after 0.1.2 shipped with `paru -S sleepless-bin` in the README and on the
# website, for a package that does not exist on the AUR -- it had been committed
# locally and never pushed, and `origin/master` had no ref to notice that with. The
# LAN plane was in the same state for a different reason. Both are exactly the failure
# this project keeps finding in itself: a documented way in that nobody walks.
#
# Offline checks (tests/docs.rs) can only prove the pages agree with each other and
# with the code. Whether a registry actually has the package is a question you have to
# go and ask.
#
#     tools/check-channels.sh            # against the latest GitHub release
#     tools/check-channels.sh 0.1.2      # against a specific version
#
# Exits non-zero if a channel is missing or behind. The LAN plane is off-estate for
# most runs, so it announces a skip when unreachable rather than failing.
set -euo pipefail

REPO=${GH_REPO:-lariocpt/sleepless}
TAP_RAW=${TAP_RAW:-https://raw.githubusercontent.com/lariocpt/homebrew-sleepless/main/Formula/sleepless.rb}
AUR_PKG=${AUR_PKG:-sleepless-bin}
PLANE=${PLANE:-https://apps.in.drlario.org}
UA='User-Agent: sleepless-check-channels'

want=${1:-}
if [ -z "$want" ]; then
    url=$(curl -fsS -o /dev/null -w '%{url_effective}' -L "https://github.com/$REPO/releases/latest")
    want=${url##*/}; want=${want#v}
fi
case "$want" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "not a version: '$want'" >&2; exit 2 ;;
esac

README=${README:-$(dirname "$0")/../README.md}

# What the README's Install block actually tells people to run.
#
# Scoped to that one fenced block rather than the whole file, because the prose around
# it discusses channels that are deliberately NOT offered -- the AUR package is written
# and ready but cannot be registered -- and a check that could not tell an offer from an
# explanation would re-arm itself on its own footnote. Deleting a line from that block
# is therefore how you retire a channel, and restoring it is how you re-arm this.
advertised() {  # <grep pattern>
    awk '/^## Install/ {f=1} f && /^```/ {c++; next} f && c==1 {print} c>1 {exit}' "$README" \
        | grep -q "$1"
}

echo "every advertised channel should serve sleepless $want"
fail=0
report() {  # <channel> <got> [note]
    if [ "$2" = "$want" ]; then
        printf '  ok      %-16s %s\n' "$1" "$2"
    elif [ -z "$2" ]; then
        printf '  MISSING %-16s %s\n' "$1" "${3:-not published at all}"
        fail=1
    else
        printf '  BEHIND  %-16s %s (want %s)\n' "$1" "$2" "$want"
        fail=1
    fi
}
skip() {  # <channel> <why>
    printf '  --      %-16s %s\n' "$1" "$2"
}

# Ask a few times before believing a "no".
#
# raw.githubusercontent.com and the crates.io index both lag their source by up to a
# minute, so running this straight after a channel bump -- which is exactly when you
# would run it -- reported the tap as having no formula at all while the GitHub API
# was already serving 0.1.3. A check that goes red for reasons unrelated to the thing
# it checks is the failure this file exists to prevent, so it is not allowed to be one.
settled() {  # <command...> -- echoes the first non-empty answer
    local out i
    for i in 1 2 3 4 5 6; do
        out=$("$@" 2>/dev/null || true)
        [ -n "$out" ] && { printf '%s' "$out"; return; }
        [ "$i" -lt 6 ] && sleep 5
    done
}

# --- GitHub Releases --------------------------------------------------------
tag=$(curl -fsS -o /dev/null -w '%{url_effective}' -L "https://github.com/$REPO/releases/latest" || true)
report "github" "${tag##*/v}"

# --- crates.io --------------------------------------------------------------
if advertised 'cargo install sleepless'; then
    crates_version() {
        curl -fsS -H "$UA" "https://crates.io/api/v1/crates/sleepless" \
            | sed -n 's/.*"max_stable_version":"\([^"]*\)".*/\1/p'
    }
    report "crates.io" "$(settled crates_version)"
else
    skip "crates.io" "not advertised in the README's Install block"
fi

# --- Homebrew tap -----------------------------------------------------------
if advertised 'brew install lariocpt/sleepless'; then
    tap_version() {
        curl -fsS "$TAP_RAW" | sed -n 's/^[[:space:]]*version[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
    }
    report "homebrew" "$(settled tap_version)" "the tap has no formula (is the repo public?)"
else
    skip "homebrew" "not advertised in the README's Install block"
fi

# --- AUR --------------------------------------------------------------------
# The RPC returns resultcount 0 for a package that was never pushed, which is
# indistinguishable from a typo until you look -- so say which it is.
if advertised "$AUR_PKG"; then
    aur_version() {
        curl -fsS -H "$UA" "https://aur.archlinux.org/rpc/v5/info?arg\[\]=$AUR_PKG" \
            | sed -n 's/.*"Version":"\([^"-]*\).*/\1/p' | head -1
    }
    report "aur" "$(settled aur_version)" "$AUR_PKG is not on the AUR (committed but never pushed?)"
else
    skip "aur" "not advertised (AUR registrations are closed; the package is ready)"
fi

# --- LAN apps plane ---------------------------------------------------------
if curl -fsS -m 8 -o /dev/null "$PLANE/index.tsv" 2>/dev/null; then
    v=$(curl -fsS -m 15 "$PLANE/index.tsv" \
        | awk -F'\t' '$1=="tool" && $2=="sleepless" && index($7,"/latest/")>0 {print $3; exit}')
    report "lan plane" "$v" "no sleepless row on the plane"
else
    printf '  SKIP    %-16s %s\n' "lan plane" "not reachable from here"
fi

if [ "$fail" != 0 ]; then
    echo "a channel the README advertises is not serving $want" >&2
    exit 1
fi
echo "every advertised channel serves sleepless $want"
