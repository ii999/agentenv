#!/usr/bin/env python3
"""Build only repository-owned source and run the isolated application lab."""

from __future__ import annotations

import argparse
import io
import subprocess
import tarfile
import tempfile
from pathlib import Path


def main() -> int:
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-only", action="store_true")
    matrix = parser.add_mutually_exclusive_group()
    matrix.add_argument("--ssh", action="store_true", help="Run the real SSH application matrix")
    matrix.add_argument("--release", action="store_true", help="Run sudo policy and SSH refusal release cases")
    args = parser.parse_args()
    dependencies = tempfile.TemporaryDirectory(prefix="agentenv-lab-dependencies-")
    vendor = Path(dependencies.name) / "vendor"
    subprocess.run([
        "cargo", "vendor", "--quiet", "--locked", "--versioned-dirs", str(vendor),
    ], cwd=repo, stdout=subprocess.DEVNULL, check=True)
    # The Docker context is an explicit source allowlist. Host Git state,
    # credentials, build caches, and local workflow artifacts never enter it.
    context = io.BytesIO()
    roots = ["Cargo.toml", "Cargo.lock", "build.rs", "src", "tests/fixtures/bin", "tests/sudo_execution_lab"]
    with tarfile.open(fileobj=context, mode="w:gz") as archive:
        archive.add(repo / "tests/sudo_execution_lab/Dockerfile", arcname="Dockerfile")
        for name in roots:
            root = repo / name
            paths = sorted(root.rglob("*")) if root.is_dir() else [root]
            for path in paths:
                if path.is_symlink():
                    raise RuntimeError("Docker source context cannot contain symlinks")
                if path.is_file() and "__pycache__" not in path.parts:
                    archive.add(path, arcname=str(path.relative_to(repo)), recursive=False)
        for path in sorted(vendor.rglob("*")):
            if path.is_symlink():
                raise RuntimeError("Vendored dependency context cannot contain symlinks")
            if path.is_file():
                archive.add(path, arcname="vendor/" + str(path.relative_to(vendor)), recursive=False)
    dependencies.cleanup()
    subprocess.run([
        "docker", "build", "--tag", "agentenv-sudo-execution:lab", "-",
    ], input=context.getvalue(), cwd=repo, check=True)
    if not args.build_only:
        invocation = [
            "docker", "run", "--rm", "--network", "none",
            "--hostname", "agentenv-sudo-lab", "--add-host", "agentenv-sudo-lab:127.0.0.1",
        ]
        if args.ssh or args.release:
            verifier = "verify_ssh.py" if args.ssh else "verify_release.py"
            invocation += ["--entrypoint", "python3", "agentenv-sudo-execution:lab", "/opt/sudo-lab/" + verifier]
        else:
            invocation += ["agentenv-sudo-execution:lab"]
        subprocess.run(invocation, cwd=repo, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
