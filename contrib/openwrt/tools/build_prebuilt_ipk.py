#!/usr/bin/env python3
"""Package a supplied ARM64 binary and this checkout's OpenWrt integration.

This helper packages but does not compile binaries. Pass --binary for a local
cross-build. Without it, the headless smoke-test path downloads the verified
upstream v0.13.1 binary, which does not include this checkout's Rust changes.
"""

from __future__ import annotations

import argparse
import ast
import gzip
import hashlib
import io
import os
from pathlib import Path
import shutil
import struct
import tarfile
import tempfile
import urllib.request


VERSION = "0.13.1"
RELEASE = "24"
ARCH = "aarch64_cortex-a53"
ARCHIVE_NAME = f"smartdns-aarch64-unknown-linux-musl-v{VERSION}.tar.gz"
ARCHIVE_URL = (
    f"https://github.com/mokeyish/smartdns-rs/releases/download/v{VERSION}/"
    f"{ARCHIVE_NAME}"
)
ARCHIVE_SHA256 = "01f0df565fe663ec93b7e5a84d54d5d0b8a216e74b3c51dba7bda84a6d0d44f8"
BUILD_TIMESTAMP = 1782572786  # v0.13.1 release timestamp

DEFAULT_POSTINST = """#!/bin/sh
[ "$IPKG_NO_SCRIPT" = "1" ] && exit 0
[ -s "${IPKG_INSTROOT}/lib/functions.sh" ] || exit 0
. "${IPKG_INSTROOT}/lib/functions.sh"
default_postinst "$0" "$@"
"""
DEFAULT_PRERM = """#!/bin/sh
[ -s "${IPKG_INSTROOT}/lib/functions.sh" ] || exit 0
. "${IPKG_INSTROOT}/lib/functions.sh"
default_prerm "$0" "$@"
"""


def copy_file(source: Path, root: Path, target: str, mode: int = 0o644) -> None:
    destination = root / target.lstrip("/")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if source.suffix == ".htm":
        # Lua LuCI's template parser leaves CR from CRLF inside generated Lua
        # strings. Windows checkouts must not ship those line endings.
        destination.write_bytes(source.read_bytes().replace(b"\r\n", b"\n"))
    else:
        shutil.copyfile(source, destination)
    os.chmod(destination, mode)


def download_binary(cache: Path) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / ARCHIVE_NAME
    if not archive.exists():
        print(f"Downloading {ARCHIVE_URL}")
        urllib.request.urlretrieve(ARCHIVE_URL, archive)

    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    if actual != ARCHIVE_SHA256:
        archive.unlink(missing_ok=True)
        raise RuntimeError(f"release SHA256 mismatch: {actual}")

    extract = cache / "release"
    binary = extract / "smartdns-aarch64-unknown-linux-musl" / "smartdns"
    if not binary.exists():
        shutil.rmtree(extract, ignore_errors=True)
        extract.mkdir(parents=True)
        with tarfile.open(archive, "r:gz") as source:
            member = source.getmember("smartdns-aarch64-unknown-linux-musl/smartdns")
            source.extract(member, extract, filter="data")
    print(f"Verified upstream binary archive: {ARCHIVE_SHA256}")
    return binary


def parse_po(path: Path) -> list[tuple[str, str]]:
    entries: list[tuple[str, str]] = []
    current: str | None = None
    msgid = ""
    msgstr = ""

    def finish() -> None:
        nonlocal msgid, msgstr
        if msgid and msgstr and msgid != msgstr:
            entries.append((msgid, msgstr))
        msgid = ""
        msgstr = ""

    for raw_line in path.read_text(encoding="utf-8").splitlines() + ["msgid \"\""]:
        line = raw_line.strip()
        if line.startswith("msgid "):
            finish()
            current = "id"
            msgid = ast.literal_eval(line[6:])
        elif line.startswith("msgstr "):
            current = "str"
            msgstr = ast.literal_eval(line[7:])
        elif line.startswith('"'):
            value = ast.literal_eval(line)
            if current == "id":
                msgid += value
            elif current == "str":
                msgstr += value
        elif not line:
            finish()
            current = None
    return entries


def u32(value: int) -> int:
    return value & 0xFFFFFFFF


def sfh_hash(data: bytes) -> int:
    length = len(data)
    value = length
    remainder = length & 3
    blocks = length >> 2
    offset = 0

    def get16(position: int) -> int:
        return data[position] | (data[position + 1] << 8)

    for _ in range(blocks):
        value = u32(value + get16(offset))
        tmp = u32((get16(offset + 2) << 11) ^ value)
        value = u32((value << 16) ^ tmp)
        offset += 4
        value = u32(value + (value >> 11))

    if remainder == 3:
        value = u32(value + get16(offset))
        value = u32(value ^ (value << 16))
        signed = data[offset + 2] if data[offset + 2] < 128 else data[offset + 2] - 256
        value = u32(value ^ (signed << 18))
        value = u32(value + (value >> 11))
    elif remainder == 2:
        value = u32(value + get16(offset))
        value = u32(value ^ (value << 11))
        value = u32(value + (value >> 17))
    elif remainder == 1:
        signed = data[offset] if data[offset] < 128 else data[offset] - 256
        value = u32(value + signed)
        value = u32(value ^ (value << 10))
        value = u32(value + (value >> 1))

    value = u32(value ^ (value << 3))
    value = u32(value + (value >> 5))
    value = u32(value ^ (value << 4))
    value = u32(value + (value >> 17))
    value = u32(value ^ (value << 25))
    return u32(value + (value >> 6))


def compile_lmo(po: Path, output: Path) -> None:
    values = bytearray()
    index: list[tuple[int, int, int, int]] = []
    for msgid, msgstr in parse_po(po):
        key = msgid.encode("utf-8")
        value = msgstr.encode("utf-8")
        offset = len(values)
        values.extend(value)
        values.extend(b"\0" * ((4 - len(value) % 4) % 4))
        index.append((sfh_hash(key), 1, offset, len(value)))

    index.sort(key=lambda entry: entry[0])
    with output.open("wb") as stream:
        stream.write(values)
        for entry in index:
            stream.write(struct.pack(">IIII", *entry))
        stream.write(struct.pack(">I", len(values)))


def tar_bytes(root: Path, executable_paths: set[str] | None = None) -> bytes:
    raw = io.BytesIO()
    executable_paths = executable_paths or set()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.GNU_FORMAT) as archive:
        root_info = tarfile.TarInfo("./")
        root_info.type = tarfile.DIRTYPE
        root_info.mode = 0o755
        root_info.uid = root_info.gid = 0
        root_info.uname = root_info.gname = "root"
        root_info.mtime = BUILD_TIMESTAMP
        archive.addfile(root_info)
        paths = sorted(root.rglob("*"))
        for path in paths:
            relative = path.relative_to(root).as_posix()
            info = archive.gettarinfo(str(path), arcname=f"./{relative}")
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = BUILD_TIMESTAMP
            if path.is_dir():
                info.mode = 0o755
                archive.addfile(info)
            else:
                info.mode = 0o755 if relative in executable_paths else 0o644
                with path.open("rb") as source:
                    archive.addfile(info, source)
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", mtime=0) as stream:
        stream.write(raw.getvalue())
    return compressed.getvalue()


def write_ipk(path: Path, data_archive: bytes, control_archive: bytes) -> None:
    raw = io.BytesIO()
    members = [
        ("./debian-binary", b"2.0\n"),
        ("./data.tar.gz", data_archive),
        ("./control.tar.gz", control_archive),
    ]
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.GNU_FORMAT) as archive:
        for name, data in members:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = BUILD_TIMESTAMP
            archive.addfile(info, io.BytesIO(data))
    with path.open("wb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", mtime=0) as stream:
            stream.write(raw.getvalue())


def package(
    output: Path,
    name: str,
    version: str,
    architecture: str,
    depends: str,
    description: str,
    data_root: Path,
    scripts: dict[str, str] | None = None,
    conffiles: list[str] | None = None,
    data_executables: set[str] | None = None,
    provides: str | None = None,
    conflicts: str | None = None,
) -> Path:
    control_root = Path(tempfile.mkdtemp(prefix="smartdns-control-"))
    try:
        data_archive = tar_bytes(data_root, data_executables)
        installed_size = len(gzip.decompress(data_archive))
        control = (
            f"Package: {name}\nVersion: {version}\nArchitecture: {architecture}\n"
            f"Maintainer: SmartDNS-rs contributors\nInstalled-Size: {installed_size}\n"
            f"Depends: {depends}\n"
        )
        if provides:
            control += f"Provides: {provides}\n"
        if conflicts:
            control += f"Conflicts: {conflicts}\n"
        control += f"Section: net\nPriority: optional\nDescription: {description}\n"
        (control_root / "control").write_text(control, encoding="utf-8", newline="\n")
        os.chmod(control_root / "control", 0o644)
        for script_name, content in (scripts or {}).items():
            target = control_root / script_name
            target.write_text(content, encoding="utf-8", newline="\n")
            os.chmod(target, 0o755)
        if conffiles:
            (control_root / "conffiles").write_text(
                "".join(f"{path}\n" for path in conffiles), encoding="utf-8", newline="\n"
            )

        result = output / f"{name}_{version}_{architecture}.ipk"
        write_ipk(
            result,
            data_archive,
            tar_bytes(control_root, set((scripts or {}).keys())),
        )
        return result
    finally:
        shutil.rmtree(control_root, ignore_errors=True)


def build(repo: Path, output: Path, cache: Path, binary: Path | None = None, variant: str = "headless") -> list[Path]:
    output.mkdir(parents=True, exist_ok=True)
    if variant == "webui" and binary is None:
        raise ValueError("WebUI packaging requires --binary built with the webui Cargo feature")
    supplied_binary = binary is not None
    binary = binary if supplied_binary else download_binary(cache)
    core_name = "smartdns-rs-webui" if variant == "webui" else "smartdns-rs"
    built: list[Path] = []

    with tempfile.TemporaryDirectory(prefix="smartdns-data-") as temporary:
        root = Path(temporary)
        copy_file(binary, root, "/usr/sbin/smartdns", 0o755)
        files = repo / "contrib/openwrt/smartdns-rs/files"
        for source in files.rglob("*"):
            if source.is_file():
                relative = source.relative_to(files).as_posix()
                mode = 0o755 if relative in {
                    "etc/init.d/smartdns",
                    "etc/uci-defaults/90-smartdns-rs",
                } else 0o644
                copy_file(source, root, "/" + relative, mode)
        if variant == "webui":
            configuration = root / "etc/config/smartdns"
            content = configuration.read_text(encoding="utf-8")
            content = content.replace("\toption port '6053'", "\toption port '6053'\n\toption webui_enable '1'\n\toption webui_bind '0.0.0.0:6080'")
            configuration.write_text(content, encoding="utf-8", newline="\n")
        built.append(
            package(
                output,
                core_name,
                f"{VERSION}-{RELEASE}",
                ARCH,
                "ca-bundle",
                f"SmartDNS-rs {variant} ({'supplied binary' if supplied_binary else 'upstream prebuilt smoke test'}).",
                root,
                conffiles=[
                    "/etc/config/smartdns",
                    "/etc/smartdns/address.conf",
                    "/etc/smartdns/blacklist-ip.conf",
                    "/etc/smartdns/custom.conf",
                    "/etc/smartdns/domain-block.list",
                    "/etc/smartdns/domain-forwarding.list",
                ],
                data_executables={
                    "usr/sbin/smartdns",
                    "etc/init.d/smartdns",
                    "etc/uci-defaults/90-smartdns-rs",
                },
                provides="smartdns, smartdns-rs" if variant == "webui" else "smartdns",
                conflicts="smartdns, smartdns-rs" if variant == "webui" else "smartdns, smartdns-rs-webui",
                scripts={"postinst": DEFAULT_POSTINST, "prerm": DEFAULT_PRERM},
            )
        )

    luci = repo / "contrib/openwrt/luci-app-smartdns-rs"
    with tempfile.TemporaryDirectory(prefix="smartdns-luci-") as temporary:
        root = Path(temporary)
        for source in (luci / "root").rglob("*"):
            if source.is_file():
                relative = source.relative_to(luci / "root").as_posix()
                mode = 0o755 if relative == "usr/libexec/smartdns-rs-call" else 0o644
                copy_file(source, root, "/" + relative, mode)
        for source in (luci / "htdocs").rglob("*"):
            if source.is_file():
                relative = source.relative_to(luci / "htdocs").as_posix()
                copy_file(source, root, "/www/" + relative)
        built.append(
            package(
                output,
                "luci-app-smartdns-rs",
                f"{VERSION}-{RELEASE}",
                "all",
                "luci-base, smartdns-rs",
                "LuCI support for SmartDNS-rs.",
                root,
                scripts={
                    "postinst": DEFAULT_POSTINST
                    + "\n[ -n \"$IPKG_INSTROOT\" ] || { rm -f /tmp/luci-indexcache; /etc/init.d/rpcd reload >/dev/null 2>&1 || true; }\nexit 0\n",
                    "prerm": DEFAULT_PRERM,
                },
                data_executables={"usr/libexec/smartdns-rs-call"},
                conflicts="luci-app-smartdns",
            )
        )

    with tempfile.TemporaryDirectory(prefix="smartdns-i18n-") as temporary:
        root = Path(temporary)
        destination = root / "usr/lib/lua/luci/i18n/smartdns-rs.zh-cn.lmo"
        destination.parent.mkdir(parents=True)
        compile_lmo(luci / "po/zh_Hans/smartdns-rs.po", destination)
        built.append(
            package(
                output,
                "luci-i18n-smartdns-rs-zh-cn",
                f"{VERSION}-{RELEASE}",
                "all",
                "luci-app-smartdns-rs",
                "Chinese translation for luci-app-smartdns-rs.",
                root,
            )
        )
    return built


def build_luci_compat(repo: Path, output: Path) -> list[Path]:
    """Package the Lua UI from this checkout, without downloading a DNS binary."""
    output.mkdir(parents=True, exist_ok=True)
    source = repo / "contrib/openwrt/luci-app-smartdns-rs-compat"
    built = []
    executables = {
        "usr/libexec/smartdns-rs-call",
        "etc/uci-defaults/90-luci-smartdns-rs-compat",
    }
    with tempfile.TemporaryDirectory(prefix="smartdns-lua-") as temporary:
        root = Path(temporary)
        for path in (source / "root").rglob("*"):
            if path.is_file():
                relative = path.relative_to(source / "root").as_posix()
                copy_file(path, root, "/" + relative, 0o755 if relative in executables else 0o644)
        built.append(package(
            output, "luci-app-smartdns-rs-compat", f"{VERSION}-{RELEASE}", "all",
            "luci-base, smartdns-rs", "Lua LuCI support for SmartDNS-rs.", root,
            data_executables=executables,
            conflicts="luci-app-smartdns, luci-app-smartdns-rs",
            scripts={
                "postinst": DEFAULT_POSTINST
                + '\n[ -n "$IPKG_INSTROOT" ] || rm -f /tmp/luci-indexcache\nexit 0\n',
                "prerm": DEFAULT_PRERM,
            },
        ))
    with tempfile.TemporaryDirectory(prefix="smartdns-lua-i18n-") as temporary:
        root = Path(temporary)
        destination = root / "usr/lib/lua/luci/i18n/smartdns.zh-cn.lmo"
        destination.parent.mkdir(parents=True)
        compile_lmo(source / "po/zh_Hans/smartdns.po", destination)
        built.append(package(
            output, "luci-i18n-smartdns-rs-compat-zh-cn", f"{VERSION}-{RELEASE}", "all",
            "luci-app-smartdns-rs-compat", "Chinese translation for the Lua LuCI UI.", root,
        ))
    return built


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("--output", type=Path, default=Path("dist/openwrt/aarch64_cortex-a53"))
    parser.add_argument("--cache", type=Path, default=Path("dist/openwrt/cache"))
    parser.add_argument("--binary", type=Path, help="Use a locally built target binary")
    parser.add_argument("--variant", choices=["headless", "webui"], default="headless")
    parser.add_argument("--luci-compat-only", action="store_true",
                        help="Package the current Lua UI and translation only; no prebuilt DNS binary")
    args = parser.parse_args()
    if args.luci_compat_only:
        artifacts = build_luci_compat(args.repo.resolve(), args.output.resolve())
    else:
        if args.variant == "webui" and args.binary is None:
            parser.error("--variant webui requires --binary built with the webui feature")
        artifacts = build(args.repo.resolve(), args.output.resolve(), args.cache.resolve(), args.binary.resolve() if args.binary else None, args.variant)
    for artifact in artifacts:
        print(f"Built {artifact} ({artifact.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
