#!/usr/bin/env python3
"""Run the CDP filling parity lab.

Builds the native client, locates a Chromium-family browser, and runs the
Node harness, which launches the browser with a temporary profile, serves the
fixtures, and runs every scenario through both clients. Evidence is written to
the output directory as evidence.json and summary.md.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

LAB = Path(__file__).resolve().parent
NATIVE_MANIFEST = LAB / "native" / "Cargo.toml"

BROWSER_CANDIDATES = {
    "Darwin": [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ],
    "Linux": ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"],
}


def find_browser(explicit: str | None) -> str:
    if explicit:
        return explicit
    for candidate in BROWSER_CANDIDATES.get(platform.system(), []):
        if os.path.isabs(candidate) and os.path.exists(candidate):
            return candidate
        found = shutil.which(candidate)
        if found:
            return found
    raise SystemExit("no Chromium-family browser found; pass --browser PATH")


def build_native(release: bool) -> Path:
    command = ["cargo", "build", "--manifest-path", str(NATIVE_MANIFEST)]
    if release:
        command.append("--release")
    subprocess.run(command, check=True, cwd=LAB / "native")
    target = LAB / "native" / "target" / ("release" if release else "debug") / "cdp-fill-native"
    if not target.exists():
        raise SystemExit(f"native client not found at {target}")
    return target


REPO = LAB.parent.parent

PRODUCT_CONFIG = """version = 1

[credentials.lab_value]
description = "CDP lab scenario value."
provider = "env"
name = "AGENTENV_LAB_VALUE"
inject_as = "LAB_VALUE"
"""


def build_product(release: bool) -> Path:
    """Builds agentenv with the debug-only test hooks the lab relies on."""
    command = ["cargo", "build", "--features", "test-keychain"]
    if release:
        raise SystemExit("the product implementation needs a debug build; drop --release")
    subprocess.run(command, check=True, cwd=REPO)
    target = REPO / "target" / "debug" / "agentenv"
    if not target.exists():
        raise SystemExit(f"product binary not found at {target}")
    return target


def write_product_config(output_dir: Path) -> Path:
    path = output_dir / "product-config.toml"
    path.write_text(PRODUCT_CONFIG)
    path.chmod(0o600)
    return path


def ensure_node_dependencies() -> None:
    for package in ("fixtures", "reference"):
        if not (LAB / package / "node_modules").exists():
            subprocess.run(["pnpm", "install", "--frozen-lockfile"], check=True, cwd=LAB / package)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", required=True, help="directory for evidence.json and summary.md")
    parser.add_argument("--browser", help="Chromium-family browser executable; auto-detected when omitted")
    parser.add_argument("--headed", action="store_true", help="show the browser window")
    parser.add_argument(
        "--implementation",
        choices=["native", "reference", "product", "both", "all"],
        default="both",
        help="both = native+reference; all = native+reference+product (the agentenv binary)",
    )
    parser.add_argument("--scenario", action="append", default=[], help="run only this scenario id; repeatable")
    parser.add_argument("--release", action="store_true", help="build the native client in release mode")
    args = parser.parse_args()

    output_dir = Path(args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    browser = find_browser(args.browser)
    ensure_node_dependencies()
    wants_native = args.implementation in ("native", "both", "all")
    wants_product = args.implementation in ("product", "all")
    native_bin = build_native(args.release) if wants_native else None
    product_bin = build_product(args.release) if wants_product else None
    product_config = write_product_config(output_dir) if wants_product else None

    command = [
        "node",
        str(LAB / "harness" / "main.mjs"),
        "--output-dir",
        str(output_dir),
        "--browser",
        browser,
        "--implementation",
        args.implementation,
    ]
    if native_bin:
        command += ["--native-bin", str(native_bin)]
    if product_bin:
        command += ["--product-bin", str(product_bin), "--product-config", str(product_config)]
    if args.headed:
        command.append("--headed")
    for scenario in args.scenario:
        command += ["--scenario", scenario]
    result = subprocess.run(command, cwd=LAB)

    toolchain = {
        "rustc": subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=False).stdout.strip(),
        "cargo": subprocess.run(["cargo", "--version"], capture_output=True, text=True, check=False).stdout.strip(),
        "python": platform.python_version(),
        "os": f"{platform.system()} {platform.release()} {platform.machine()}",
    }
    evidence_path = output_dir / "evidence.json"
    if evidence_path.exists():
        evidence = json.loads(evidence_path.read_text())
        evidence["toolchain"] = toolchain
        evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
