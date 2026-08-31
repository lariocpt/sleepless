#!/usr/bin/env python3
"""Build a release archive that any rebuild of the same tag reproduces byte for byte.

Publishing a checksum is a promise that the bytes can be checked. It was not one:
the previous archives were built by `tar acf`, which records the build time, the
building user and whatever umask the machine had, so two builds of one commit
produced two different sha256 values and the "verify this download" story stopped at
"trust the file you just downloaded". A Homebrew formula and an AUR PKGBUILD both pin
those hashes, so re-running the release workflow silently invalidated them.

Everything that varies is pinned here: member order, timestamps, ownership, modes,
and the gzip header. Archiving is done in Python rather than tar(1) because the
normalisation flags are GNU-only and the macOS runners ship bsdtar -- the exact shape
of bug that shipped a hollow artifact in another project on this estate.

    tools/package.py --target x86_64-unknown-linux-musl
    tools/package.py --target x86_64-pc-windows-msvc --tag v0.1.1 --out dist
"""

import argparse
import datetime
import gzip
import hashlib
import io
import os
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = "sleepless"
EXTRA = ["README.md", "LICENSE"]

# A fixed point in time for every archive member. SOURCE_DATE_EPOCH overrides it, so
# a distribution that wants its own convention can have one.
EPOCH = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
# zip cannot express anything before 1980.
ZIP_TIME = (1980, 1, 1, 0, 0, 0)


def die(msg: str) -> "None":
    sys.exit(f"package.py: {msg}")


def cargo_version() -> str:
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text())
    if not m:
        die("could not read version from Cargo.toml")
    return m.group(1)


def check_toolchain() -> None:
    """The pin that makes the promise keepable.

    Reproducibility is per-compiler: a different rustc emits different code, so a
    rebuild that matches has to use the rustc the release used. .rust-version records
    it, and this refuses to build with anything else unless told to.
    """
    want = (ROOT / ".rust-version").read_text().strip()
    got = subprocess.run(
        ["rustc", "--version"], capture_output=True, text=True, check=True
    ).stdout.split()
    have = got[1] if len(got) > 1 else "?"
    if have != want:
        if os.environ.get("ALLOW_TOOLCHAIN_MISMATCH") == "1":
            print(f"package.py: warning: rustc {have}, .rust-version says {want}")
            return
        die(
            f"rustc is {have} but .rust-version pins {want}; the archive would not "
            f"reproduce. Use `rustup run {want} tools/package.py ...`, or set "
            f"ALLOW_TOOLCHAIN_MISMATCH=1 if you do not need a comparable build."
        )


def build_env(target: str) -> "dict[str, str]":
    """Make the compiler's output independent of where the build happened.

    A release binary embeds the source path of every panic site. Unremapped, a CI
    container and a local clone produce different bytes for identical source, so the
    published checksum could never be checked against a rebuild. `trim-paths` is the
    eventual Cargo answer and is still unstable, so this does it by hand.

    CARGO_ENCODED_RUSTFLAGS rather than RUSTFLAGS: the separator is \x1f, so a home
    directory with a space in it does not silently split one flag into two.
    """
    env = os.environ.copy()
    cargo_home = Path(env.get("CARGO_HOME") or (Path.home() / ".cargo")).resolve()
    remaps = [
        f"--remap-path-prefix={ROOT}=/sleepless",
        f"--remap-path-prefix={cargo_home}=/cargo",
    ]
    # The MSVC linker stamps the PE header with the wall clock, so two builds of one
    # commit differ by the seconds between them -- which is exactly how the first
    # rehearsal of this release failed, on both Windows targets and nothing else.
    # /Brepro makes link.exe emit a fixed timestamp instead. verify_deterministic()
    # below checks it actually took.
    if target.endswith("-pc-windows-msvc"):
        remaps.append("-Clink-arg=/Brepro")

    existing = env.get("CARGO_ENCODED_RUSTFLAGS")
    parts = (existing.split("\x1f") if existing else []) + remaps
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(parts)
    # RUSTFLAGS would otherwise win over the encoded form and undo all of the above.
    env.pop("RUSTFLAGS", None)
    return env


def build(target: str, tool: str) -> Path:
    cmd = [tool, "build", "--release", "--locked", "--target", target]
    print("package.py:", " ".join(cmd))
    subprocess.run(cmd, cwd=ROOT, check=True, env=build_env(target))
    exe = BIN + (".exe" if "windows" in target else "")
    out = ROOT / "target" / target / "release" / exe
    if not out.is_file():
        die(f"{out} was not produced")
    return out


# Any Unix time in this window is a real clock reading rather than a hash: nothing
# reproducible would land there by design, and a build cannot predate the project.
CLOCK_WINDOW = (1577836800, 4102444800)  # 2020-01-01 .. 2100-01-01


def verify_deterministic(binary: Path, target: str) -> None:
    """Refuse to package a Windows binary that carries the time it was built.

    A silent regression here is the worst kind: the archive still builds, still
    installs, still runs, and only the reproducibility claim quietly stops being
    true -- which nobody notices until someone tries to check a checksum.

    `/Brepro` does not write a constant. link.exe replaces the timestamp with a hash
    of the output, so the test is that the value cannot be a clock reading, not that
    it equals any particular number. The authority on reproducibility is still the
    release workflow's rebuild-and-compare; this is the cheap check that runs first
    and names the cause.
    """
    if not target.endswith("-pc-windows-msvc"):
        return
    data = binary.read_bytes()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        die(f"{binary} does not look like a PE image")
    stamp = struct.unpack_from("<I", data, pe + 8)[0]
    if CLOCK_WINDOW[0] <= stamp <= CLOCK_WINDOW[1]:
        when = datetime.datetime.fromtimestamp(stamp, datetime.UTC)
        die(
            f"{binary.name} has PE TimeDateStamp {stamp} (0x{stamp:08x}), which reads "
            f"as {when:%Y-%m-%d %H:%M:%SZ} -- the linker stamped it with the clock, so "
            f"this archive would not reproduce. Did /Brepro stop being passed?"
        )
    print(f"package.py: {binary.name} carries no build timestamp (0x{stamp:08x})")


def members(binary: Path) -> "list[tuple[str, Path, int]]":
    """(name in the archive, file on disk, mode) -- sorted, so order is not luck."""
    items = [(binary.name, binary, 0o755)]
    for name in EXTRA:
        p = ROOT / name
        if not p.is_file():
            die(f"{name} is missing; the archive is supposed to carry it")
        items.append((name, p, 0o644))
    return sorted(items, key=lambda i: i[0])


def write_tar_gz(dest: Path, items) -> None:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.GNU_FORMAT) as tar:
        for name, path, mode in items:
            info = tarfile.TarInfo(name)
            info.size = path.stat().st_size
            info.mtime = EPOCH
            info.mode = mode
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.type = tarfile.REGTYPE
            with path.open("rb") as fh:
                tar.addfile(info, fh)
    # mtime=0 keeps the gzip header itself out of the hash.
    with dest.open("wb") as out:
        with gzip.GzipFile(fileobj=out, mode="wb", compresslevel=9, mtime=0) as gz:
            gz.write(raw.getvalue())


def write_zip(dest: Path, items) -> None:
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for name, path, mode in items:
            info = zipfile.ZipInfo(name, date_time=ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3  # unix, so the mode below is meaningful
            info.external_attr = (mode & 0xFFFF) << 16
            z.writestr(info, path.read_bytes())


def self_test() -> int:
    """Exercise verify_deterministic without needing a Windows build.

    /Brepro's replacement value is a hash, so the rule here is "this cannot be a
    clock reading" rather than any particular constant -- which is a rule worth
    testing, because getting it wrong in either direction is silent: too strict and
    every Windows release fails, too loose and the check stops meaning anything.
    """
    import tempfile

    def fake_pe(stamp: int) -> Path:
        data = bytearray(512)
        data[0x3C:0x40] = struct.pack("<I", 0x80)
        data[0x80:0x84] = b"PE\0\0"
        data[0x88:0x8C] = struct.pack("<I", stamp)
        p = Path(tempfile.mkstemp(suffix=".exe")[1])
        p.write_bytes(bytes(data))
        return p

    cases = [
        (0x2A7E0E6F, True, "the hash link.exe /Brepro actually wrote"),
        (0xFFFFFFFF, True, "the constant some linkers write instead"),
        (1788202353, False, "a real clock reading, from an unfixed build"),
        (CLOCK_WINDOW[0], False, "the near edge of the clock window"),
        (CLOCK_WINDOW[1], False, "the far edge of the clock window"),
    ]
    failures = 0
    for stamp, want_ok, why in cases:
        exe = fake_pe(stamp)
        try:
            verify_deterministic(exe, "x86_64-pc-windows-msvc")
            got_ok = True
        except SystemExit:
            got_ok = False
        finally:
            exe.unlink()
        if got_ok != want_ok:
            print(f"  FAIL 0x{stamp:08x} ({why}): expected ok={want_ok}", file=sys.stderr)
            failures += 1
    # And a non-Windows target is not inspected at all.
    exe = fake_pe(1788202353)
    verify_deterministic(exe, "x86_64-unknown-linux-musl")
    exe.unlink()
    if failures:
        return 1
    print("package.py: self-test ok")
    return 0


def main() -> int:
    if "--self-test" in sys.argv[1:]:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--target", required=True)
    ap.add_argument("--tag", help="release tag; defaults to v<Cargo.toml version>")
    ap.add_argument("--out", default="dist")
    ap.add_argument("--build-tool", default="cargo", choices=["cargo", "cross"])
    ap.add_argument(
        "--skip-build",
        action="store_true",
        help="archive an existing target/<target>/release build",
    )
    ap.add_argument("--no-check-toolchain", action="store_true")
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="check the reproducibility guards and exit; CI runs this",
    )
    args = ap.parse_args()

    tag = args.tag or f"v{cargo_version()}"
    if not tag.startswith("v"):
        die(f"tag {tag!r} should start with 'v'; the archive names embed it")
    # A `-rc.N` tag rehearses the release it is a candidate for, and the binary it
    # packages reports that release's version -- so the BASE is what must match.
    # Comparing the whole tag made every prerelease unbuildable, which would have been
    # found the first time the rehearsal the release workflow documents was actually
    # attempted.
    base = tag[1:].split("-", 1)[0]
    if base != cargo_version():
        die(f"tag {tag} claims {base}, Cargo.toml says {cargo_version()}")

    if not args.no_check_toolchain:
        check_toolchain()

    exe = BIN + (".exe" if "windows" in args.target else "")
    binary = (
        ROOT / "target" / args.target / "release" / exe
        if args.skip_build
        else build(args.target, args.build_tool)
    )
    if not binary.is_file():
        die(f"{binary} is missing (--skip-build with nothing built?)")
    verify_deterministic(binary, args.target)

    out = ROOT / args.out
    out.mkdir(parents=True, exist_ok=True)
    stem = f"{BIN}-{args.target}-{tag}"
    ext = ".zip" if "windows" in args.target else ".tar.gz"
    archive = out / (stem + ext)
    if archive.exists():
        archive.unlink()

    items = members(binary)
    (write_zip if ext == ".zip" else write_tar_gz)(archive, items)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    # Written with the bare file name and no directory component, because that is
    # what `sha256sum -c` reads out of it -- a path in here makes the file useless
    # anywhere but the directory it was generated in.
    (out / (stem + ext + ".sha256")).write_text(f"{digest}  {archive.name}\n")
    print(f"{digest}  {archive.name}")
    shown = archive.relative_to(ROOT) if archive.is_relative_to(ROOT) else archive
    print(f"package.py: {shown} ({archive.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    if shutil.which("rustc") is None and not {"--skip-build", "--self-test"} & set(sys.argv):
        die("rustc is not on PATH")
    raise SystemExit(main())
