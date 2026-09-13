#!/usr/bin/env python3
"""Static contract checks between Rust, procd/UCI and LuCI.

This intentionally uses only the Python standard library so it can run in the
OpenWrt package CI job before an SDK is downloaded.
"""

from __future__ import annotations

import ast
import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
OPENWRT = ROOT / "contrib" / "openwrt"
CORE = OPENWRT / "smartdns-rs"
INIT = CORE / "files" / "etc" / "init.d" / "smartdns"
LUCI = (
    OPENWRT
    / "luci-app-smartdns-rs"
    / "htdocs"
    / "luci-static"
    / "resources"
    / "view"
    / "smartdns"
    / "smartdns.js"
)
LUCI_LOG = LUCI.with_name("log.js")
LUCI_ZH_HANS = (
    OPENWRT
    / "luci-app-smartdns-rs"
    / "po"
    / "zh_Hans"
    / "smartdns-rs.po"
)
LOG_HELPER = (
    OPENWRT
    / "luci-app-smartdns-rs"
    / "root"
    / "usr"
    / "libexec"
    / "smartdns-rs-call"
)


def fail(message: str) -> None:
    print(f"contract error: {message}", file=sys.stderr)
    raise SystemExit(1)


def load(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")


def check_json() -> None:
    json_files = [
        OPENWRT
        / "luci-app-smartdns-rs"
        / "root"
        / "usr"
        / "share"
        / "luci"
        / "menu.d"
        / "luci-app-smartdns-rs.json",
        OPENWRT
        / "luci-app-smartdns-rs"
        / "root"
        / "usr"
        / "share"
        / "rpcd"
        / "acl.d"
        / "luci-app-smartdns-rs.json",
    ]
    for path in json_files:
        try:
            json.loads(load(path))
        except json.JSONDecodeError as error:
            fail(f"invalid JSON in {path.relative_to(ROOT)}: {error}")


def rust_directives() -> set[str]:
    parser = load(ROOT / "src" / "config" / "parser" / "mod.rs")
    directives = set(re.findall(r'config\("([A-Za-z0-9_-]+)"\)', parser))
    directives.update(
        {
            "bind",
            "bind-udp",
            "bind-tcp",
            "bind-tls",
            "bind-https",
            "bind-quic",
            "bind-h3",
            "server",
            "server-udp",
            "server-tcp",
            "server-tls",
            "server-https",
            "server-quic",
            "server-h3",
            "group-end",
        }
    )
    return {item.lower() for item in directives}


def emitted_directives(init: str) -> set[str]:
    directives = set(
        re.findall(
            r"\bconf_append(?:_file|_bind)?\s+([A-Za-z][A-Za-z0-9_-]*)",
            init,
        )
    )
    # These are selected dynamically from a fixed shell case/loop.
    directives.update(
        {
            "server",
            "server-tcp",
            "server-tls",
            "server-https",
            "server-quic",
            "server-h3",
            "speed-check-mode",
            "response-mode",
            "cache-size",
            "rr-ttl",
            "rr-ttl-min",
            "rr-ttl-max",
            "rr-ttl-reply-max",
            "dualstack-ip-selection",
            "prefetch-domain",
            "serve-expired",
            "mdns-lookup",
            "group-end",
        }
    )
    return {item.lower() for item in directives}


def check_directives(init: str) -> None:
    unknown = sorted(emitted_directives(init) - rust_directives())
    if unknown:
        fail("init script emits directives not parsed by Rust: " + ", ".join(unknown))


def check_luci_options(init: str) -> None:
    luci = load(LUCI)
    options = set(
        re.findall(
            r"\.option\([^\n]*?form\.[A-Za-z]+,\s*'([A-Za-z0-9_-]+)'",
            luci,
        )
    )
    options.update(
        re.findall(
            r"\.taboption\([^\n]*?form\.[A-Za-z]+,\s*'([A-Za-z0-9_-]+)'",
            luci,
        )
    )
    # These options are constructed from fixed tables instead of literal option calls.
    options.update(
        {
            "rr_ttl",
            "rr_ttl_min",
            "rr_ttl_max",
            "rr_ttl_reply_max",
            "seconddns_no_speed_check",
            "seconddns_no_rule_addr",
            "seconddns_no_rule_nameserver",
            "seconddns_no_rule_ipset",
            "seconddns_no_rule_soa",
            "seconddns_no_dualstack_selection",
            "seconddns_no_cache",
            "seconddns_force_aaaa_soa",
            "seconddns_force_https_soa",
            "whitelist_ip",
            "blacklist_ip",
            "ignore_ip",
            "bogus_nxdomain",
        }
    )
    # Buttons, upload pickers, text-file editors and display metadata deliberately
    # bypass UCI. Their handlers write files or invoke the init script directly.
    ui_only = {
        "address_conf",
        "blacklist_conf",
        "custom_conf",
        "desc",
        "domain_block_list",
        "domain_forwarding_list",
        "upload_conf_file",
        "upload_list_file",
        "upload_other_file",
    }
    options = {
        option
        for option in options
        if not option.startswith("_") and option not in ui_only
    }
    consumed = set(
        re.findall(
            r'config_get(?:_bool)?\s+\S+\s+["\']?\$section["\']?\s+["\']?([A-Za-z0-9_-]+)',
            init,
        )
    )
    consumed.update(
        re.findall(
            r'config_list_foreach\s+["\']?\$section["\']?\s+["\']?([A-Za-z0-9_-]+)',
            init,
        )
    )
    # Fixed shell loops generate these option names at runtime.
    consumed.update(
        {
            "cache_size",
            "dualstack_ip_selection",
            "mdns_lookup",
            "prefetch_domain",
            "response_mode",
            "rr_ttl",
            "rr_ttl_min",
            "rr_ttl_max",
            "rr_ttl_reply_max",
            "serve_expired",
            "speed_check_mode",
            "seconddns_no_rule_addr",
            "seconddns_no_rule_nameserver",
            "seconddns_no_rule_ipset",
            "seconddns_no_rule_soa",
            "seconddns_no_dualstack_selection",
            "seconddns_no_cache",
            "seconddns_force_aaaa_soa",
            "seconddns_force_https_soa",
        }
    )
    missing = sorted(options - consumed)
    if missing:
        fail("LuCI exposes options not consumed by init script: " + ", ".join(missing))


def po_entries(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    for block in re.split(r"\r?\n\s*\r?\n", load(path)):
        fields: dict[str, str] = {}
        current: str | None = None
        for line in block.splitlines():
            match = re.match(r"^(msgid|msgstr)\s+(\".*\")$", line)
            if match:
                current = match.group(1)
                fields[current] = ast.literal_eval(match.group(2))
            elif current and re.match(r'^\".*\"$', line):
                fields[current] += ast.literal_eval(line)
            elif line and not line.startswith("#"):
                current = None
        if fields.get("msgid"):
            entries[fields["msgid"]] = fields.get("msgstr", "")
    return entries


def check_translations() -> None:
    sources = [load(LUCI), load(LUCI_LOG)]
    msgids: set[str] = set()
    literal = re.compile(r"_\(\s*('(?:\\.|[^'\\])*'|\"(?:\\.|[^\"\\])*\")\s*\)")
    for source in sources:
        if re.search(r"_\(\s*(?!['\"])", source):
            fail("LuCI contains dynamically generated gettext keys")
        for match in literal.finditer(source):
            msgids.add(ast.literal_eval(match.group(1)))

    translations = po_entries(LUCI_ZH_HANS)
    missing = sorted(msgid for msgid in msgids if not translations.get(msgid))
    if missing:
        fail("LuCI zh_Hans translations are missing: " + ", ".join(missing))


def check_luci_validation() -> None:
    luci = load(LUCI)
    required = {
        "download file names": "so.validate = validateDownloadName",
        "decimal and hexadecimal packet marks": "o.validate = validatePacketMark",
    }
    for purpose, marker in required.items():
        if marker not in luci:
            fail(f"LuCI does not validate {purpose}")


def check_luci_compat() -> None:
    compat = OPENWRT / "luci-app-smartdns-rs-compat"
    lua_root = compat / "root/usr/lib/lua/luci"
    lua = "\n".join(load(path) for path in (lua_root / "model/cbi/smartdns").glob("*.lua"))
    # Include the names from the fixed JS tables, as well as literal options.
    js = load(LUCI)
    options = set(re.findall(
        r"(?:taboption|option)\([^\n]*?form\.\w+,\s*'([\w-]+)'", js
    ))
    options.update(re.findall(r"\[\s*'(rr_ttl\w*|whitelist_ip|blacklist_ip|ignore_ip|bogus_nxdomain)'", js))
    options.update("seconddns_" + name for name in re.findall(
        r"\[\s*'(no_speed_check|no_rule_\w+|no_dualstack_selection|no_cache|force_\w+_soa)'", js
    ))
    options = {name for name in options if not name.startswith("_") and name != "seconddns_"}
    lua_options = set(re.findall(r':(?:taboption|option)\([^\n]*?\w+,\s*"([\w-]+)"', lua))
    if missing := sorted(options - lua_options):
        fail("Lua LuCI is missing JS configuration options: " + ", ".join(missing))

    translations = po_entries(compat / "po/zh_Hans/smartdns.po")
    for path in lua_root.rglob("*"):
        if path.suffix not in {".lua", ".htm"}:
            continue
        source = load(path)
        labels = re.findall(r'(?:translate|_)\("([^"\n]+)"\)', source)
        labels.extend(re.findall(r"<%:([^%]+)%>", source))
        if missing := sorted(label for label in labels if not translations.get(label)):
            fail("Lua LuCI translations are missing: " + ", ".join(missing))

    for relative in ("usr/libexec/smartdns-rs-call", "usr/share/rpcd/acl.d/luci-app-smartdns-rs.json"):
        if load(compat / "root" / relative) != load(OPENWRT / "luci-app-smartdns-rs/root" / relative):
            fail("Lua/JS shared helper or ACL has diverged: " + relative)


def check_features() -> None:
    cargo = load(ROOT / "Cargo.toml")
    package = load(CORE / "Makefile")
    build_script = load(ROOT / "build.rs")
    feature_match = re.search(r"^openwrt\s*=\s*\[([^]]+)\]", cargo, re.MULTILINE)
    if not feature_match:
        fail("Cargo.toml does not define the openwrt feature set")
    cargo_features = set(re.findall(r'"([^" ]+)"', feature_match.group(1)))
    package_match = re.search(r"^RUST_PKG_FEATURES:=(.+)$", package, re.MULTILINE)
    if not package_match:
        fail("OpenWrt Makefile has no RUST_PKG_FEATURES")
    package_features = set(package_match.group(1).strip().split(","))
    if cargo_features != package_features:
        fail(
            "Cargo/OpenWrt feature sets differ: "
            f"Cargo-only={sorted(cargo_features - package_features)}, "
            f"package-only={sorted(package_features - cargo_features)}"
        )
    rust_include = "include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk"
    sse_condition = "ifeq ($(RUSTC_TARGET_ARCH),i586-unknown-linux-musl)"
    if (
        sse_condition not in package
        or package.index(sse_condition) < package.index(rust_include)
        or "CARGO_RUSTFLAGS+=-Ctarget-feature=+sse,+sse2" not in package
    ):
        fail("OpenWrt package does not enable SSE/SSE2 after resolving the Rust target")
    if "cc::Build" in build_script or "bindgen::" in build_script:
        fail("project build.rs must not compile a C implementation")
    for directory in (ROOT / "src", ROOT / "include"):
        if any(directory.rglob("*.c")):
            fail("the project implementation must remain Rust-only")
    for required in ("VARIANT:=headless", "VARIANT:=webui", "ifeq ($(BUILD_VARIANT),webui)", "RUST_PKG_FEATURES+=webui"):
        if required not in package:
            fail("missing independent OpenWrt build variant: " + required)


def check_runtime_safety(init: str) -> None:
    helper = load(LOG_HELPER)
    if "conf_append user nobody" not in init:
        fail("generated configuration does not explicitly drop to the OpenWrt nobody user")
    if 'readlink -f "$log_file"' not in helper:
        fail("LuCI log helper does not resolve symlinks before reading")
    if "/var/log/smartdns/*" not in helper:
        fail("LuCI log helper is not restricted to the managed log directory")


def main() -> None:
    init = load(INIT)
    check_json()
    check_directives(init)
    check_luci_options(init)
    check_translations()
    check_luci_validation()
    check_luci_compat()
    check_features()
    check_runtime_safety(init)
    print("OpenWrt Rust/UCI/LuCI contract: OK")


if __name__ == "__main__":
    main()
