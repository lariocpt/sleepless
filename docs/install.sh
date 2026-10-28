#!/bin/sh
# install.sh — the public installer for sleepless.
#
#     curl -fsSL https://lariocpt.github.io/sleepless/install.sh | sh
#
# It picks the release build for this machine, downloads that archive and the
# .sha256 beside it, verifies the checksum, extracts the binary and installs it.
# That is all it does.
#
# Unlike color-terminal's bootstrap — which verifies the artifact and hands it to
# the installer compiled inside it — there is no installer inside this artifact:
# sleepless is just the binary, so the placement lives here, in the one file that
# can verify before it executes. Which is why the checksum check has no skip
# branch: a hash this machine could not compute is a refusal, not a warning.
#
# POSIX sh on purpose: this is run by whatever /bin/sh the reader has — dash, ash,
# busybox, bash in posix mode. No arrays, no [[ ]], no `local`, no pipefail.
#
# EVERYTHING LIVES IN FUNCTIONS AND THE LAST LINE IS `main "$@"`. That is not
# style. The script is executed straight off a socket; if the connection dies
# halfway, a top-to-bottom script would run the half that arrived. sh cannot call
# main until it has read the closing brace, so a truncated download does nothing
# at all.
#
# Options:
#   vX.Y.Z, --tag=vX.Y.Z        install one release, not the latest    [SLEEPLESS_VERSION]
#   --prefix=DIR, --prefix DIR  where the binary goes (default ~/.local/bin) [SLEEPLESS_PREFIX]
#   --target=TRIPLE             override build detection, e.g. x86_64-unknown-linux-gnu [SLEEPLESS_TARGET]
#   --source=github|apps        where to resolve from (default github) [SLEEPLESS_SOURCE]
#   --download-only=PATH        fetch, verify and extract to PATH; install nothing
#   --print-url                 resolve and print the download URL; fetch nothing
#   -n, --dry-run               resolve and report what would be installed; write nothing
#   -h, --help                  this text
#
# Also honoured: SLEEPLESS_REPO (owner/name), SLEEPLESS_DOWNLOAD_BASE (a URL or
# file:// dir holding the archives and their .sha256 sidecars — a mirror, or a
# rehearsal), APPS_URL (root of the LAN mirror, for --source=apps).
set -eu

SL_REPO_DEFAULT='lariocpt/sleepless'

sl_say()  { printf 'sleepless: %s\n' "$*"; }
sl_warn() { printf 'sleepless: %s\n' "$*" >&2; }
sl_die()  { sl_warn "$*"; exit 1; }
sl_have() { command -v "$1" >/dev/null 2>&1; }

# A heredoc, not a sed range over "$0": piped through sh there is no $0 to read.
sl_usage() {
    cat <<'EOF'
install.sh — install sleepless from its published release.

    curl -fsSL https://lariocpt.github.io/sleepless/install.sh | sh
    curl -fsSL https://lariocpt.github.io/sleepless/install.sh | sh -s -- --dry-run
    curl -fsSL https://lariocpt.github.io/sleepless/install.sh | sh -s -- v0.1.2

Options:
  vX.Y.Z, --tag=vX.Y.Z        install one release, not the latest    [SLEEPLESS_VERSION]
  --prefix=DIR, --prefix DIR  where the binary goes, default ~/.local/bin  [SLEEPLESS_PREFIX]
  --target=TRIPLE             override build detection              [SLEEPLESS_TARGET]
  --source=github|apps        where to resolve from, default github [SLEEPLESS_SOURCE]
  --download-only=PATH        fetch, verify and extract to PATH; install nothing
  --print-url                 resolve and print the download URL; fetch nothing
  -n, --dry-run               resolve and report; write nothing
  -h, --help                  this text

Also honoured: SLEEPLESS_REPO, SLEEPLESS_DOWNLOAD_BASE, APPS_URL.
The download is verified against the release's .sha256 before anything runs.
EOF
}

# Invoked only from trap strings, which shellcheck cannot see through — as
# SC2317 on the apt 0.9.0 the runners use and SC2329 on the modern one.
# shellcheck disable=SC2317,SC2329
sl_cleanup() { if [ -n "${SL_TMP:-}" ]; then rm -rf "$SL_TMP"; fi; }

# The scheme allowlist is https for the published path and is widened only when the
# caller has already asked for something else — a LAN mirror, a file:// rehearsal.
# What makes the transport safe to relax is the sha256 check, which is never skipped.
sl_fetch() {                                  # <url> <dest>
    case "$1" in
        https://*) proto='=https' ;;
        *)         proto='=https,http,file' ;;
    esac
    if sl_have curl; then
        curl -fsSL --proto "$proto" --retry 3 --retry-delay 1 -o "$2" "$1"
    elif sl_have wget; then
        # --https-only is GNU wget's; busybox has no such flag, hence the probe in
        # main. SL_WGET_HTTPS is deliberately empty or exactly one flag.
        # shellcheck disable=SC2086
        case "$1" in
            file://*)  sl_die 'wget cannot fetch file:// URLs — install curl to use a local mirror' ;;
            https://*) wget -q $SL_WGET_HTTPS -O "$2" "$1" ;;
            *)         wget -q -O "$2" "$1" ;;
        esac
    else
        sl_die 'need curl or wget'
    fi
}

# Fail closed. A checksum you cannot compute is not a checksum, and the job of this
# script is to put an executable on somebody's PATH. There is no branch that skips it.
sl_sha256() {                                 # <file> -> hex on stdout
    if   sl_have sha256sum; then sha256sum "$1" | cut -d' ' -f1
    elif sl_have shasum;    then shasum -a 256 "$1" | cut -d' ' -f1
    elif sl_have openssl;   then openssl dgst -sha256 "$1" | sed 's/.*= *//'
    else return 1
    fi
}

# Pin the tag ONCE, then fetch the archive and its sidecar from that tag's own
# directory. Two separate trips through /releases/latest/ can straddle a release
# and pair the old checksum with the new file — and then tell the user their
# download was tampered with. One HEAD request, no token, no API budget.
sl_latest_tag() {                             # <owner/repo> -> tag on stdout
    url="https://github.com/$1/releases/latest"
    if sl_have curl; then
        loc=$(curl -fsSIL --proto '=https' -o /dev/null -w '%{url_effective}' "$url") || return 1
    else
        loc=$(wget --spider -S "$url" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -1)
    fi
    loc=${loc%%\?*}; loc=${loc%/}
    tag=${loc##*/}
    case "$tag" in v[0-9]*) printf '%s' "$tag" ;; *) return 1 ;; esac
}

# The release publishes one archive per target and the musl builds are fully
# static, so Linux always gets musl — no glibc floor to get wrong — and macOS
# gets the target its own architecture runs on. Anything else is honestly
# refused rather than guessed at: an installer that picks the wrong binary is
# worse than the manual download it replaces.
sl_detect_target() {
    repo=${SLEEPLESS_REPO:-$SL_REPO_DEFAULT}
    os=$(uname -s); arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)  printf 'x86_64-unknown-linux-musl' ;;
                aarch64|arm64) printf 'aarch64-unknown-linux-musl' ;;
                *) sl_die "no sleepless build for Linux/$arch — https://github.com/$repo/releases has the list" ;;
            esac ;;
        Darwin)
            case "$arch" in
                x86_64)        printf 'x86_64-apple-darwin' ;;
                arm64)         printf 'aarch64-apple-darwin' ;;
                *) sl_die "no sleepless build for macOS/$arch — https://github.com/$repo/releases has the list" ;;
            esac ;;
        MINGW*|MSYS*|CYGWIN*)
            sl_die 'on Windows, download the .zip from https://github.com/lariocpt/sleepless/releases (this installer is for Linux and macOS)' ;;
        *)  sl_die "unsupported system '$os' — https://github.com/lariocpt/sleepless/releases has every build" ;;
    esac
}

sl_resolve_github() {                         # -> SL_URL, SL_SUMS_URL
    repo=${SLEEPLESS_REPO:-$SL_REPO_DEFAULT}
    if [ -n "${SLEEPLESS_DOWNLOAD_BASE:-}" ]; then
        # A mirror directory has no /latest/ to resolve, so the version has to be
        # named there, not guessed at.
        [ "$SL_VERSION" != latest ] || \
            sl_die 'SLEEPLESS_DOWNLOAD_BASE needs an explicit version (--tag=X.Y.Z)'
        base=${SLEEPLESS_DOWNLOAD_BASE%/}
    else
        if [ "$SL_VERSION" = latest ]; then
            tag=$(sl_latest_tag "$repo") \
                || sl_die "could not resolve the latest release of $repo — is one published?"
            # The archive is named for the resolved version, never for 'latest'.
            SL_VERSION=${tag#v}
        else
            tag="v$SL_VERSION"                # GitHub tags carry the v; versions do not
        fi
        base="https://github.com/$repo/releases/download/$tag"
    fi
    SL_ARCHIVE="sleepless-$SL_TARGET-v$SL_VERSION.tar.gz"
    SL_URL="$base/$SL_ARCHIVE"
    SL_SUMS_URL="$SL_URL.sha256"
}

# The LAN mirror is resolved through its index rather than a guessed path: the row
# whose path goes via latest/ is the only one a client is meant to read, and it
# carries the sha256 inline, so this plane needs no separate checksum file. What
# it mirrors is the single x86_64 musl binary — byte-identical to that release
# build, which is the whole point of a mirror that never rebuilds — so anyone on
# another architecture must not be silently handed the wrong bytes.
sl_resolve_apps() {                           # -> SL_URL, SL_WANT, SL_VERSION, sl_flat
    apps="${APPS_URL:-https://apps.in.drlario.org}"
    sl_fetch "$apps/index.tsv" "$SL_TMP/index.tsv" || sl_die "cannot reach the LAN mirror at $apps"
    row=$(awk -F'\t' -v v="$SL_VERSION" '
        $1=="tool" && $2=="sleepless" && index($7,"/latest/")>0 &&
        (v=="latest" || $3==v) { print $5 "|" $7 "|" $3; exit }' "$SL_TMP/index.tsv")
    [ -n "$row" ] || sl_die "no sleepless $SL_VERSION on the LAN mirror at $apps"
    case "$SL_TARGET" in
        x86_64-unknown-linux-musl) ;;
        *) sl_die "the LAN mirror carries only the x86_64 musl build — drop --source=apps for $SL_TARGET" ;;
    esac
    SL_WANT=${row%%|*}
    rest=${row#*|}
    SL_URL="$apps/${rest%%|*}"
    SL_VERSION=${rest##*|}                    # 'latest' becomes the concrete version
    SL_SUMS_URL=
    sl_flat=1                                 # the mirrored file IS the binary
}

# The only exit from a mismatch is a refusal. What SL_WANT is a hash OF differs by
# source — the archive on github, the mirrored binary on apps — so the caller
# hands over the file the claim describes.
sl_verify() {                                 # <file whose hash SL_WANT is>
    got=$(sl_sha256 "$1") \
        || sl_die 'no sha256sum, shasum or openssl here — refusing to install an unverified binary'
    # An empty hash is a hasher that failed, not a file that mismatched; say which.
    [ -n "$got" ] || sl_die 'could not hash the download (hasher failed) — refusing to continue'
    [ "$SL_WANT" = "$got" ] || sl_die "CHECKSUM MISMATCH — do not run the downloaded file
  expected $SL_WANT
  got      $got"
}

# The sidecar is `sha256sum` format ("hex  filename"). Take the first field and
# require it to look like a hash before trusting it against the computed one —
# an HTML error page served with a 200 must not parse as a checksum.
sl_want_from_sums() {
    sl_fetch "$SL_SUMS_URL" "$SL_TMP/$SL_ARCHIVE.sha256" \
        || sl_die "cannot fetch $SL_SUMS_URL — is that release published?"
    SL_WANT=$(awk 'NR==1 && length($1)==64 && $1 !~ /[^0-9a-fA-F]/ { print $1 }' \
        "$SL_TMP/$SL_ARCHIVE.sha256")
    [ -n "$SL_WANT" ] || sl_die "$SL_ARCHIVE.sha256 does not contain a sha256"
}

main() {
    SL_VERSION=${SLEEPLESS_VERSION:-latest}
    sl_prefix=${SLEEPLESS_PREFIX:-}
    sl_source=${SLEEPLESS_SOURCE:-github}
    SL_TARGET=${SLEEPLESS_TARGET:-}
    sl_dlonly=''; sl_printurl=''; sl_dry=''; sl_flat=''

    # The version list is ours alone — sleepless has no installer to pass flags on
    # to — so an unrecognised argument is an error, not a silent passthrough.
    while [ "$#" -gt 0 ]; do
        case "$1" in
            v[0-9]*)           SL_VERSION=${1#v} ;;
            [0-9]*.[0-9]*)     SL_VERSION=$1 ;;
            --tag=*)           SL_VERSION=${1#--tag=}; SL_VERSION=${SL_VERSION#v} ;;
            --prefix=*)        sl_prefix=${1#--prefix=} ;;
            --prefix)          [ "$#" -gt 1 ] || sl_die '--prefix needs a value'
                               sl_prefix=$2; shift ;;
            --target=*)        SL_TARGET=${1#--target=} ;;
            --source=*)        sl_source=${1#--source=} ;;
            --download-only=*) sl_dlonly=${1#--download-only=} ;;
            --print-url)       sl_printurl=1 ;;
            -n|--dry-run)      sl_dry=1 ;;
            -h|--help)         sl_usage; exit 0 ;;
            *)                 sl_die "unknown option '$1' — try --help" ;;
        esac
        shift
    done
    case "$SL_VERSION" in
        latest) ;;
        [0-9]*.[0-9]*.[0-9]*) ;;
        *) sl_die "not a version: '$SL_VERSION' (want X.Y.Z or latest)" ;;
    esac

    [ -n "$sl_prefix" ] || sl_prefix="$HOME/.local/bin"
    # Absolute, and no trailing slash: the file we finally write is compared and
    # reported by string. Not `pwd -P` — resolving symlinks would change the
    # spelling of the very file we are about to replace.
    case "$sl_prefix" in /*) ;; *) sl_prefix="$PWD/$sl_prefix" ;; esac
    while :; do case "$sl_prefix" in */) sl_prefix=${sl_prefix%/} ;; *) break ;; esac; done

    SL_WGET_HTTPS=
    if ! sl_have curl && sl_have wget && wget --help 2>&1 | grep -q -- --https-only; then
        SL_WGET_HTTPS=--https-only
    fi
    [ -n "$SL_TARGET" ] || SL_TARGET=$(sl_detect_target)

    # The archive name is built from the version, so resolving latest has to happen
    # before anything is named — the tag pins once, both fetches live under it.
    SL_TMP=$(mktemp -d "${TMPDIR:-/tmp}/sleepless.XXXXXX") || sl_die 'mktemp failed'
    # Clean up, then RE-RAISE. A trap that only cleans up swallows the signal:
    # Ctrl-C after the download would fall through to a completed install with
    # exit 0.
    trap 'sl_cleanup' EXIT
    trap 'sl_cleanup; trap - INT EXIT;  kill -INT  $$' INT
    trap 'sl_cleanup; trap - TERM EXIT; kill -TERM $$' TERM
    trap 'sl_cleanup; trap - HUP EXIT;  kill -HUP  $$' HUP

    SL_WANT=''
    case "$sl_source" in
        github) sl_resolve_github ;;
        apps)   sl_resolve_apps ;;
        *)      sl_die "unknown --source '$sl_source' (want: github, apps)" ;;
    esac
    if [ -n "$sl_printurl" ]; then printf '%s\n' "$SL_URL"; exit 0; fi

    if [ -n "$sl_dry" ]; then
        sl_say "would install sleepless $SL_VERSION for $SL_TARGET"
        sl_say "  from  $SL_URL"
        sl_say "  into  $sl_prefix/sleepless"
        exit 0
    fi

    # With --download-only there is no release-wide truth to fetch, but the
    # sidecar is cheap and it is what tells us the file is complete.
    [ -n "$SL_SUMS_URL" ] && sl_want_from_sums

    sl_say "downloading $SL_URL"
    if [ -n "$sl_flat" ]; then
        # apps mode mirrors the bare binary; the index row's hash is about that file.
        sl_fetch "$SL_URL" "$SL_TMP/sleepless" || sl_die "download failed: $SL_URL"
        sl_verify "$SL_TMP/sleepless"
    else
        sl_fetch "$SL_URL" "$SL_TMP/$SL_ARCHIVE" || sl_die "download failed: $SL_URL"
        # The sidecar's claim is about the ARCHIVE, so it is verified before the
        # unpack: a bad archive never gets the chance to produce a binary at all.
        sl_verify "$SL_TMP/$SL_ARCHIVE"
        sl_have tar || sl_die 'need tar to unpack the release archive'
        tar xzf "$SL_TMP/$SL_ARCHIVE" -C "$SL_TMP" sleepless \
            || sl_die "could not unpack $SL_ARCHIVE"
    fi
    chmod 0755 "$SL_TMP/sleepless"             # release assets arrive without the bit
    sl_say "verified $("$SL_TMP/sleepless" --version 2>/dev/null || echo 'unknown version')"

    if [ -n "$sl_dlonly" ]; then
        dir=$(dirname "$sl_dlonly")
        mkdir -p "$dir" || sl_die "cannot create $dir"
        { cp "$SL_TMP/sleepless" "$sl_dlonly.tmp.$$" && chmod 0755 "$sl_dlonly.tmp.$$" \
          && mv -f "$sl_dlonly.tmp.$$" "$sl_dlonly"; } \
            || sl_die "cannot write $sl_dlonly"
        sl_say "wrote $sl_dlonly"
        exit 0
    fi

    # Verified first, executable second, in place third: nothing unverified ever
    # reaches a stable path, and a half-written file never replaces a good one —
    # the copy lands on a new inode under a temp name and rename(2) swaps it.
    mkdir -p "$sl_prefix" || sl_die "cannot create $sl_prefix"
    if [ -f "$sl_prefix/sleepless" ]; then
        # </dev/null and head: whatever is already there might not be sleepless at
        # all, and an interactive stand-in must not hang the install.
        old=$("$sl_prefix/sleepless" --version </dev/null 2>/dev/null | sed 's/^sleepless //' | head -n1 || true)
        if [ -n "$old" ]; then
            sl_say "replacing sleepless $old at $sl_prefix/sleepless"
        else
            sl_say "replacing an existing sleepless at $sl_prefix/sleepless"
        fi
    fi
    { cp "$SL_TMP/sleepless" "$sl_prefix/.sleepless.tmp.$$" \
        && chmod 0755 "$sl_prefix/.sleepless.tmp.$$" \
        && mv -f "$sl_prefix/.sleepless.tmp.$$" "$sl_prefix/sleepless"; } \
        || sl_die "cannot write $sl_prefix/sleepless"
    sl_say "installed $sl_prefix/sleepless"

    # A warning, not an error: the binary is there either way.
    case ":${PATH:-}:" in
        *":$sl_prefix:"*) ;;
        *) sl_warn "$sl_prefix is not on your PATH. To run it by name, add:
      export PATH=\"$sl_prefix:\$PATH\"" ;;
    esac
    exit 0
}

main "$@"
