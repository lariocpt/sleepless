// nosleep — build the release binary and publish it to the LAN artifact plane.
//
// Target machines then install a verified prebuilt binary instead of compiling:
//
//     curl -fsSL https://apps.in.drlario.org/install.sh | bash -s -- nosleep
//
// WHY THE BUILD IMAGE IS DEBIAN BOOKWORM AND NOT THIS HOST'S TOOLCHAIN
// glibc symbol versioning is a FLOOR, not a ceiling: a binary records the minimum version
// each imported symbol needs, and glibc keeps old versions alive forever. So build-old,
// run-new is safe and the reverse is not. Measured on this estate:
//
//     built in bookworm (glibc 2.36)  -> floor GLIBC_2.35  -> runs everywhere
//     built on CachyOS  (glibc 2.44)  -> floor GLIBC_2.39  -> will NOT start on bookworm
//
// The apps plane is a general LAN channel whose consumers are "whatever machine ran
// install.sh", so the artifact must not carry a hidden compatibility contract.
//
// musl-static would remove the floor question entirely and this dep graph is pure Rust, so
// it is a one-line change (`--target x86_64-unknown-linux-musl` plus `rustup target add`).
// Deliberately not done yet: it would be an untested toolchain, and the ldd gate below
// would need inverting.
pipeline {
    agent any
    options {
        disableConcurrentBuilds()
        buildDiscarder(logRotator(numToKeepStr: '20'))
    }
    environment {
        BUILDER = 'rust:1.97-slim-bookworm'
        TOOL    = 'nosleep'
    }
    stages {
        stage('Preflight') {
            steps {
                sh '''
                    set -eu
                    test -w /srv/apps || { echo "/srv/apps not writable"; exit 1; }
                    test -x /opt/publish/bin/apps-publish || { echo "apps-publish not mounted"; exit 1; }
                    test -f Cargo.lock || { echo "no Cargo.lock — the build uses --locked"; exit 1; }
                '''
            }
        }
        stage('Version') {
            steps {
                sh '''
                    set -eu
                    # sed, not node: node is not installed in the Jenkins container.
                    BASE=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\\([^"]*\\)".*/\\1/p' Cargo.toml | head -1)
                    [ -n "$BASE" ] || { echo "could not parse version from Cargo.toml"; exit 1; }
                    SHA=$(git rev-parse --short HEAD)
                    echo "APPS_VERSION=${BASE}+${SHA}" > version.env
                    echo "BASE_VERSION=${BASE}"       >> version.env
                    cat version.env
                '''
            }
        }
        stage('Build') {
            steps {
                sh '''
                    set -eu
                    . ./version.env
                    rm -rf out && mkdir -p out

                    # Named docker volumes for the cargo caches. This is NOT the forbidden
                    # workspace bind-mount: a named volume is resolved by the daemon itself, so
                    # it behaves identically from inside this container. Cold build is ~25 s;
                    # warm is a few seconds.
                    #
                    # CARGO_HOME is MOVED to /cargo rather than mounting a volume over
                    # /usr/local/cargo/registry, because cargo's cross-process lock lives at
                    # $CARGO_HOME/.package-cache — mounting only the registry would leave two
                    # concurrent jobs sharing a registry while each held a private lock.
                    CID=$(docker create -w /w \
                        -e CARGO_HOME=/cargo -e CARGO_TERM_COLOR=never -e CARGO_NET_RETRY=5 \
                        -v cargo-home:/cargo \
                        -v nosleep-cargo-target:/w/target \
                        "$BUILDER" sh -c '
                            set -eu
                            cd /w
                            cargo build --release --locked
                            install -m0755 target/release/nosleep /w/out-nosleep
                        ')
                    trap 'docker rm -f "$CID" >/dev/null 2>&1 || true' EXIT

                    # docker cp both ways — never -v "$PWD:/w": the workspace lives in the
                    # jenkins_home NAMED VOLUME, so that host path does not exist as the daemon
                    # resolves it.
                    docker cp "$PWD/." "$CID:/w" >/dev/null
                    docker start -a "$CID"

                    # Published as the file name `nosleep`: install.sh installs a tool BY FILE
                    # NAME, so this basename becomes the command on every target machine.
                    docker cp "$CID:/w/out-nosleep" "$WORKSPACE/out/nosleep"
                    chmod 0755 out/nosleep
                    ls -lh out/nosleep
                '''
            }
        }
        stage('Gate') {
            steps {
                sh '''
                    set -eu
                    . ./version.env

                    # It has to actually run, and report the version we think it is. `file` is
                    # not installed in this container, so executing it IS the ELF check.
                    got=$(./out/nosleep --version | tr -d "\\r")
                    echo "reported: $got"
                    case "$got" in
                        *"$BASE_VERSION"*) : ;;
                        *) echo "FAIL: binary reports '$got', expected it to contain '$BASE_VERSION'"; exit 1 ;;
                    esac

                    # Portability floor. Fail if the build image ever changes to something whose
                    # glibc is newer than the oldest machine we serve.
                    floor=$(objdump -T out/nosleep 2>/dev/null | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1 || echo unknown)
                    echo "glibc floor: $floor"
                    case "$floor" in
                        GLIBC_2.3[0-6]|GLIBC_2.2*|unknown) : ;;
                        *) echo "FAIL: glibc floor $floor is newer than the bookworm baseline (2.36)."
                           echo "      Did the build image change? A binary built on a newer glibc will not start on older machines."
                           exit 1 ;;
                    esac
                '''
            }
        }
        stage('Publish') {
            steps {
                sh '''
                    set -eu
                    . ./version.env
                    # apps-publish repoints `latest`, prunes to KEEP_VERSIONS and reindexes.
                    /opt/publish/bin/apps-publish bin "$TOOL" "$APPS_VERSION" "$WORKSPACE/out/nosleep"
                '''
            }
        }
        stage('Verify') {
            steps {
                sh '''
                    set -eu
                    . ./version.env

                    # Assert the /latest/ row specifically. apps-reindex emits the concrete
                    # version row whether or not `latest` was minted, but install.sh only ever
                    # reads rows whose path goes through latest/ — so checking the concrete row
                    # can pass while no client can see the artifact.
                    awk -F'\\t' -v v="$APPS_VERSION" \
                        '$1=="tool" && $2=="nosleep" && $3==v && index($7,"/latest/")>0 {x++} END{exit !x}' \
                        /srv/apps/index.tsv \
                        || { echo "FAIL: no /latest/ row for nosleep $APPS_VERSION"; exit 1; }

                    # And end to end, the way a machine actually gets it.
                    curl -fsSL https://apps.in.drlario.org/install.sh | bash -s -- --list | grep -q nosleep \
                        || { echo "FAIL: install.sh does not list nosleep"; exit 1; }
                    echo "published nosleep $APPS_VERSION"
                '''
            }
        }
    }
}
