#!/usr/bin/env python3
"""Build only repository-owned source and run the isolated application lab.

By default the image compiles the checkout from vendored dependencies in a
throwaway build stage. With --cached-build the same pinned Rust image instead
builds the checkout with the repository's target/lab directory and the host
Cargo registry mounted, so a caller that keeps those directories between runs
(CI) compiles only what changed. Either way the binaries are built by the
image the Dockerfile pins, so they load on the lab's glibc.
"""

from __future__ import annotations

import argparse
import io
import os
import subprocess
import tarfile
import tempfile
from pathlib import Path


def main() -> int:
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument(
        "--cached-build",
        action="store_true",
        help="build in the pinned Rust image with target/lab and the Cargo registry mounted",
    )
    matrix = parser.add_mutually_exclusive_group()
    matrix.add_argument("--ssh", action="store_true", help="Run the real SSH application matrix")
    matrix.add_argument("--release", action="store_true", help="Run sudo policy and SSH refusal release cases")
    args = parser.parse_args()
    # The Docker context is an explicit allowlist. Host Git state,
    # credentials, build caches, and local workflow artifacts never enter it.
    # It is gzip-compressed because Docker recognizes a stdin context by its
    # compression magic; an uncompressed tarfile archive starts with a PAX
    # header that Docker's sniffing does not accept and reads as a Dockerfile.
    context = io.BytesIO()
    with tarfile.open(fileobj=context, mode="w:gz", compresslevel=1) as archive:
        archive.add(repo / "tests/sudo_execution_lab/Dockerfile", arcname="Dockerfile")
        add_tree(archive, repo, ["tests/sudo_execution_lab"])
        if args.cached_build:
            binaries = cached_build(repo)
            for name in BINARIES:
                binary = binaries / name
                if not binary.is_file() or binary.is_symlink():
                    raise SystemExit(f"{binary} is not a regular file")
                archive.add(binary, arcname="bin/" + name, recursive=False, filter=root_owned)
        else:
            add_tree(archive, repo, ["Cargo.toml", "Cargo.lock", "build.rs", "src", "tests/fixtures/bin"])
            with tempfile.TemporaryDirectory(prefix="agentenv-lab-dependencies-") as dependencies:
                vendor = Path(dependencies) / "vendor"
                subprocess.run([
                    "cargo", "vendor", "--quiet", "--locked", "--versioned-dirs", str(vendor),
                ], cwd=repo, stdout=subprocess.DEVNULL, check=True)
                for path in sorted(vendor.rglob("*")):
                    if path.is_symlink():
                        raise RuntimeError("Vendored dependency context cannot contain symlinks")
                    if path.is_file():
                        archive.add(path, arcname="vendor/" + str(path.relative_to(vendor)), recursive=False)
    subprocess.run([
        "docker", "build", "--tag", "agentenv-sudo-execution:lab",
        "--build-arg", "BINARIES=" + ("prebuilt" if args.cached_build else "source"), "-",
    ], input=context.getvalue(), cwd=repo, check=True, env={**os.environ, "DOCKER_BUILDKIT": "1"})
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


BINARIES = ("agentenv", "agentenv-sudo-helper", "agentenv-ssh-askpass")
# Stripped, debuginfo-free binaries keep the helper within the 64 MiB
# deployment upload bound; the lab exercises behaviour, not symbols.
BUILD_ENVIRONMENT = {"CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_DEV_STRIP": "true"}


def build_image(repo: Path) -> str:
    """The pinned Rust image of the Dockerfile's source stage."""
    for line in (repo / "tests/sudo_execution_lab/Dockerfile").read_text().splitlines():
        words = line.split()
        if len(words) == 4 and words[0] == "FROM" and words[2] == "AS" and words[3] == "source":
            return words[1]
    raise RuntimeError("the lab Dockerfile names no source stage")


def cached_build(repo: Path) -> Path:
    """Builds the lab binaries in the pinned image as the invoking user.

    Only the host's Cargo download caches are shared; the host's Cargo
    configuration and installed tools stay out of the container.
    """
    target = repo / "target" / "lab"
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    caches = [cargo_home / "registry", cargo_home / "git"]
    for directory in [target, *caches]:
        directory.mkdir(parents=True, exist_ok=True)
    invocation = [
        "docker", "run", "--rm",
        "--user", f"{os.getuid()}:{os.getgid()}",
        "--tmpfs", "/cargo:mode=1777",
        "--volume", f"{repo}:/src:ro",
        "--volume", f"{target}:/target",
        "--env", "CARGO_HOME=/cargo",
        "--env", "CARGO_TARGET_DIR=/target",
        "--env", "HOME=/tmp",
        "--workdir", "/src",
    ]
    for cache in caches:
        invocation += ["--volume", f"{cache}:/cargo/{cache.name}"]
    for name, value in BUILD_ENVIRONMENT.items():
        invocation += ["--env", f"{name}={value}"]
    invocation += [build_image(repo), "cargo", "build", "--locked", "--bins", "--features", "test-support"]
    subprocess.run(invocation, cwd=repo, check=True)
    return target / "debug"


def root_owned(info: tarfile.TarInfo) -> tarfile.TarInfo:
    """Gives a prebuilt binary the root ownership and mode the source stage
    produces; the sudo transport refuses a helper that root does not own."""
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    info.mode = 0o755
    return info


def add_tree(archive: tarfile.TarFile, repo: Path, roots: list[str]) -> None:
    """Adds every regular file under the repository-relative roots."""
    for name in roots:
        root = repo / name
        paths = sorted(root.rglob("*")) if root.is_dir() else [root]
        for path in paths:
            if path.is_symlink():
                raise RuntimeError("Docker source context cannot contain symlinks")
            if path.is_file() and "__pycache__" not in path.parts:
                archive.add(path, arcname=str(path.relative_to(repo)), recursive=False)


if __name__ == "__main__":
    raise SystemExit(main())
