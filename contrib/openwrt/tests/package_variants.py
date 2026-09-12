#!/usr/bin/env python3
"""Check the actual IPK payload/metadata for both build variants."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("packaging", ROOT / "contrib/openwrt/tools/build_prebuilt_ipk.py")
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


def contents(package: Path, name: str) -> tarfile.TarFile:
    with tarfile.open(package, "r:*") as archive:
        return tarfile.open(fileobj=io.BytesIO(archive.extractfile("./" + name).read()), mode="r:gz")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="smartdns-variant-test-") as directory:
        directory = Path(directory)
        names = []
        for variant in ("headless", "webui"):
            binary = directory / ("fixture-" + variant)
            payload = ("package fixture: " + variant).encode()
            binary.write_bytes(payload)
            packages = packaging.build(ROOT, directory / variant, directory / "cache", binary, variant)
            core = packages[0]
            names.append(core.name)
            name = "smartdns-rs-webui" if variant == "webui" else "smartdns-rs"
            other = "smartdns-rs" if variant == "webui" else "smartdns-rs-webui"
            with contents(core, "control.tar.gz") as archive:
                control = archive.extractfile("./control").read().decode()
                assert f"Package: {name}\n" in control
                assert f"Conflicts: smartdns, {other}\n" in control
                assert ("Provides: smartdns, smartdns-rs\n" in control) == (variant == "webui")
            with contents(core, "data.tar.gz") as archive:
                assert archive.extractfile("./usr/sbin/smartdns").read() == payload
                assert archive.getmember("./usr/sbin/smartdns").mode == 0o755
                config = archive.extractfile("./etc/config/smartdns").read().decode()
                assert ("option webui_enable '1'" in config) == (variant == "webui")
                assert ("option webui_bind '0.0.0.0:6080'" in config) == (variant == "webui")
        assert names[0] != names[1], "variant packages must never overwrite one another"
    print("OpenWrt independent variant package payloads: OK")


if __name__ == "__main__":
    main()
