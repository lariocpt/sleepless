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


# IMAGE_DEBUG_TYPE_REPRO. link.exe /Brepro adds a debug directory entry of this type,
# which is a fact about the file rather than a guess about a number.
DEBUG_TYPE_REPRO = 16


def _rva_to_offset(data: bytes, pe: int, rva: int) -> "int | None":
    """Where a virtual address lives in the file, via the section table."""
    n_sections = struct.unpack_from("<H", data, pe + 6)[0]
    opt_size = struct.unpack_from("<H", data, pe + 20)[0]
    sections = pe + 24 + opt_size
    for i in range(n_sections):
        sh = sections + i * 40
        va = struct.unpack_from("<I", data, sh + 12)[0]
        raw_size = struct.unpack_from("<I", data, sh + 16)[0]
        raw_ptr = struct.unpack_from("<I", data, sh + 20)[0]
        if va <= rva < va + max(raw_size, 1):
            return raw_ptr + (rva - va)
    return None


def has_brepro_marker(data: bytes) -> bool:
    """Was this PE linked with /Brepro?

    Two earlier versions of this check guessed from the timestamp value and were
    wrong in both directions: first it demanded 0xffffffff and link.exe wrote a hash,
    then the "is this a clock reading" window was wide enough that a legitimate hash
    (0xc60ff8d4, which reads as 2075) landed inside it. The linker records the fact
    directly, so ask it instead of inferring it.
    """
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0":
        die("not a PE image")
    magic = struct.unpack_from("<H", data, pe + 24)[0]
    dd = pe + 24 + (112 if magic == 0x20B else 96)  # PE32+ vs PE32
    n_dd = struct.unpack_from("<I", data, dd - 4)[0]
    if n_dd < 7:
        return False
    rva, size = struct.unpack_from("<II", data, dd + 6 * 8)  # index 6 = Debug
    if not rva or not size:
        return False
    off = _rva_to_offset(data, pe, rva)
    if off is None:
        return False
    for i in range(size // 28):
        entry_type = struct.unpack_from("<I", data, off + i * 28 + 12)[0]
        if entry_type == DEBUG_TYPE_REPRO:
            return True
    return False


def verify_deterministic(binary: Path, target: str) -> None:
    """Refuse to package a Windows binary that carries the time it was built.

    A silent regression here is the worst kind: the archive still builds, still
    installs, still runs, and only the reproducibility claim quietly stops being
    true -- which nobody notices until someone tries to check a checksum. The
    authority on reproducibility is still the release workflow's rebuild-and-compare;
    this is the cheap check that runs first and names the cause.
    """
    if not target.endswith("-pc-windows-msvc"):
        return
    data = binary.read_bytes()
    if not has_brepro_marker(data):
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        stamp = struct.unpack_from("<I", data, pe + 8)[0]
        die(
            f"{binary.name} has no /Brepro marker in its debug directory, so its "
            f"PE TimeDateStamp (0x{stamp:08x}) is the build clock and this archive "
            f"would not reproduce. Did -Clink-arg=/Brepro stop being passed?"
        )
    print(f"package.py: {binary.name} is linked /Brepro, so it carries no build clock")


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
    """Exercise the Windows guard without needing a Windows build.

    Worth having as a test rather than a comment: this check has been wrong twice,
    both times in the assertion rather than the flag. Getting it wrong is silent in
    both directions -- too strict and every Windows release fails at build, too loose
    and it stops meaning anything.
    """
    import tempfile

    def synthetic_pe(with_repro: bool) -> bytes:
        """A PE32+ image with one section and a debug directory, and nothing else."""
        pe = 0x80
        sect_rva, sect_off, sect_size = 0x1000, 0x400, 0x200
        dbg_rva = sect_rva
        n_entries = 2 if with_repro else 1
        data = bytearray(0x600)
        struct.pack_into("<I", data, 0x3C, pe)
        data[pe : pe + 4] = b"PE\0\0"
        struct.pack_into("<H", data, pe + 4, 0x8664)          # Machine
        struct.pack_into("<H", data, pe + 6, 1)               # NumberOfSections
        struct.pack_into("<I", data, pe + 8, 0xC60FF8D4)      # the 2075-looking hash
        struct.pack_into("<H", data, pe + 20, 240)            # SizeOfOptionalHeader
        struct.pack_into("<H", data, pe + 24, 0x20B)          # PE32+
        dd = pe + 24 + 112
        struct.pack_into("<I", data, dd - 4, 16)              # NumberOfRvaAndSizes
        struct.pack_into("<II", data, dd + 6 * 8, dbg_rva, n_entries * 28)
        sh = pe + 24 + 240
        data[sh : sh + 8] = b".rdata\0\0"
        struct.pack_into("<I", data, sh + 12, sect_rva)
        struct.pack_into("<I", data, sh + 16, sect_size)
        struct.pack_into("<I", data, sh + 20, sect_off)
        # Entry 0 is a CODEVIEW record; entry 1, when present, is the REPRO marker.
        struct.pack_into("<I", data, sect_off + 12, 2)
        if with_repro:
            struct.pack_into("<I", data, sect_off + 28 + 12, DEBUG_TYPE_REPRO)
        return bytes(data)

    failures = 0
    for with_repro, want_ok, why in [
        (True, True, "linked with /Brepro"),
        (False, False, "no marker, so the timestamp is the clock"),
    ]:
        exe = Path(tempfile.mkstemp(suffix=".exe")[1])
        exe.write_bytes(synthetic_pe(with_repro))
        try:
            verify_deterministic(exe, "x86_64-pc-windows-msvc")
            got_ok = True
        except SystemExit:
            got_ok = False
        finally:
            exe.unlink()
        if got_ok != want_ok:
            print(f"  FAIL {why}: expected ok={want_ok}", file=sys.stderr)
            failures += 1

    # The value of the timestamp is deliberately not the test: 0xc60ff8d4 above is a
    # real hash link.exe produced, and an earlier version of this check rejected it
    # for reading as the year 2075.
    exe = Path(tempfile.mkstemp(suffix=".exe")[1])
    exe.write_bytes(synthetic_pe(False))
    verify_deterministic(exe, "x86_64-unknown-linux-musl")  # not inspected at all
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
