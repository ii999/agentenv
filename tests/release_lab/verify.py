"""Verify packaged installers and self-update before publishing a release."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import ctypes
import functools
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
from typing import Iterator


REPO = Path(__file__).resolve().parents[2]


def run(argv: list[str], env: dict[str, str], cwd: Path, *, input: str | None = None) -> str:
    result = subprocess.run(
        argv, env=env, cwd=cwd, input=input, text=True, capture_output=True, timeout=180
    )
    if result.returncode:
        raise RuntimeError(
            f"Command failed ({result.returncode}): {argv!r}\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


@contextmanager
def signing_keychain(root: Path, env: dict[str, str], debug: bool) -> Iterator[None]:
    if sys.platform != "darwin":
        yield
        return
    if not debug and os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError(
            "Production macOS verification changes the default Keychain only on a disposable "
            "GitHub runner. Local test-support builds use --debug-keychain."
        )
    security = ctypes.CDLL("/System/Library/Frameworks/Security.framework/Security")
    foundation = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    security.SecKeychainCreate.argtypes = [
        ctypes.c_char_p, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_ubyte,
        ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ]
    security.SecKeychainCreate.restype = ctypes.c_int32
    security.SecKeychainDelete.argtypes = [ctypes.c_void_p]
    security.SecKeychainDelete.restype = ctypes.c_int32
    foundation.CFRelease.argtypes = [ctypes.c_void_p]
    keychain = ctypes.c_void_p()
    path = root / "release-verification.keychain"
    password = b"agentenv-release-fixture"
    status = security.SecKeychainCreate(
        os.fsencode(path), len(password), password, 0, None, ctypes.byref(keychain)
    )
    if status:
        raise RuntimeError(f"Cannot create the disposable Keychain: OSStatus {status}")
    previous = None
    try:
        if debug:
            env.update({"AGENTENV_TEST_NATIVE_KEYCHAIN": str(path), "AGENTENV_TEST_KEYCHAIN_NO_UI": "1"})
        else:
            previous = run(["/usr/bin/security", "default-keychain", "-d", "user"], env, root).strip().strip('"')
            run(["/usr/bin/security", "default-keychain", "-d", "user", "-s", str(path)], env, root)
        yield
    finally:
        try:
            if previous:
                run(["/usr/bin/security", "default-keychain", "-d", "user", "-s", previous], env, root)
        finally:
            status = security.SecKeychainDelete(keychain)
            foundation.CFRelease(keychain)
            if status:
                raise RuntimeError(f"Cannot delete the disposable Keychain: OSStatus {status}")


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@contextmanager
def mirror(root: Path, archive: Path, tag: str) -> Iterator[str]:
    releases = root / "releases"
    download = releases / "download" / tag
    latest = releases / "latest/download"
    download.mkdir(parents=True)
    latest.mkdir(parents=True)
    shutil.copyfile(archive, download / archive.name)
    sums = f"{digest(archive)}  {archive.name}\n"
    (download / "SHA256SUMS").write_text(sums, encoding="utf-8")
    (latest / "SHA256SUMS").write_text(sums, encoding="utf-8")
    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), functools.partial(QuietHandler, directory=str(root))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/releases"
    finally:
        server.shutdown()
        server.server_close()


def installation_environment(root: Path, base: dict[str, str]) -> dict[str, str]:
    root.mkdir()
    env = base | {"XDG_DATA_HOME": str(root / "data"), "AGENTENV_FILE": str(root / "config.toml")}
    (root / "config.toml").write_text(
        'version = 1\n[credentials.fixture]\ndescription = "Synthetic release fixture."\n'
        f'provider = "keychain"\nservice = "agentenv-test-release-{root.name}"\n'
        'account = "fixture"\ninject_as = "AGENTENV_SYNTHETIC_TOKEN"\n', encoding="utf-8"
    )
    (root / "config.toml").chmod(0o600)
    return env


def install(root: Path, env: dict[str, str], tag: str | None) -> None:
    if sys.platform == "win32":
        argv = ["pwsh", "-NoProfile", "-File", str(REPO / "install.ps1"),
                "-NoSkill", "-InstallDir", str(root / "bin")]
        if tag:
            argv += ["-Version", tag]
    else:
        argv = ["/bin/bash", str(REPO / "install.sh"), "--no-skill", "--dir", str(root / "bin")]
        if tag:
            argv += ["--version", tag]
    run(argv, env, root)


def binary(root: Path, name: str = "agentenv") -> str:
    return str(root / "bin" / (name + (".exe" if sys.platform == "win32" else "")))


def verify_bundle(root: Path, env: dict[str, str], tag: str) -> None:
    version = tag.removeprefix("v")
    for name, flag, expected in [
        ("agentenv", "--version", f"agentenv {version}"),
        ("agentenv-sudo-helper", "--identity", f"agentenv-sudo-helper 1 {version}"),
        ("agentenv-ssh-askpass", "--identity", f"agentenv-ssh-askpass 1 {version}"),
    ]:
        actual = run([binary(root, name), flag], env, root).strip()
        if actual != expected:
            raise RuntimeError(f"Incorrect installed identity: {actual!r}; expected {expected!r}")
        if sys.platform == "darwin":
            run(["/usr/bin/codesign", "--verify", "--strict", binary(root, name)], env, root)


def helper_snapshot(root: Path, env: dict[str, str]) -> tuple[str, ...]:
    if sys.platform != "darwin":
        return ()
    report = json.loads(run([binary(root), "keychain", "status", "--json"], env, root))
    if report["helper"]["status"] != "ready":
        raise RuntimeError(f"The credential helper is not ready: {report['helper']}")
    helper = Path(report["helper"]["details"]["helper"])
    expected = root / "data/agentenv/macos-keychain/agentenv-keychain-helper"
    if helper.resolve() != expected.resolve():
        raise RuntimeError("The fixture did not isolate its active credential helper")
    return (digest(helper), digest(helper.parent / "helper.toml"),
            digest(helper.parent / "signing.toml"), report["helper"]["details"]["certificate_sha1"])


def credential_roundtrip(root: Path, env: dict[str, str]) -> None:
    run([binary(root), "credential", "set", "fixture"], env, root, input="synthetic-release-fixture\n")
    run([binary(root), "credential", "check", "fixture"], env, root)


def update(root: Path, env: dict[str, str], tag: str) -> None:
    run([binary(root), "update", "--force", "--version", tag, "--no-skill"], env, root)


def extract_executable(archive: Path, tag: str, target: str, name: str, destination: Path) -> None:
    with tarfile.open(archive) as bundle:
        member = bundle.getmember(f"agentenv-{tag}-{target}/{name}")
        if not member.isfile():
            raise RuntimeError("The archive does not contain a regular executable")
        source = bundle.extractfile(member)
        if source is None:
            raise RuntimeError("The archived executable cannot be read")
        with source, destination.open("wb") as output:
            shutil.copyfileobj(source, output)
        destination.chmod(0o755)



def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--debug-keychain", action="store_true", help="Local test-support builds only; avoids changing the default Keychain")
    args = parser.parse_args()
    archive = args.archive.resolve(strict=True)
    expected = f"agentenv-{args.tag}-{args.target}." + ("zip" if sys.platform == "win32" else "tar.gz")
    if archive.name != expected:
        parser.error(f"expected archive named {expected}")
    base_env = {key: value for key, value in os.environ.items() if not key.startswith("AGENTENV_")}
    base_env["AGENTENV_NO_PROJECT"] = "1"
    with tempfile.TemporaryDirectory(prefix="agentenv-release-") as temporary:
        root = Path(temporary)
        if args.debug_keychain:
            if sys.platform != "darwin":
                parser.error("--debug-keychain is specific to macOS test-support builds")
            probe = root / "test-build-probe"
            extract_executable(archive, args.tag, args.target, "agentenv", probe)
            # Release builds ignore debug-only Keychain selectors. Reject them
            # before any fixture can accidentally use the real login Keychain.
            run([str(probe), "keychain", "delete-fixture", "--help"], base_env, root)
        with mirror(root, archive, args.tag) as base_url:
            base_env["AGENTENV_RELEASE_BASE_URL"] = base_url
            fresh = root / "fresh"
            env = installation_environment(fresh, base_env)
            with signing_keychain(fresh, env, args.debug_keychain):
                install(fresh, env, None)
                verify_bundle(fresh, env, args.tag)
                snapshot = helper_snapshot(fresh, env)
                if sys.platform == "darwin":
                    credential_roundtrip(fresh, env)
                update(fresh, env, args.tag)
                verify_bundle(fresh, env, args.tag)
                if helper_snapshot(fresh, env) != snapshot:
                    raise RuntimeError("Ordinary update changed the active helper or local identity")
                install(fresh, env, args.tag)
                if helper_snapshot(fresh, env) != snapshot:
                    raise RuntimeError("Reinstallation changed the active helper or local identity")
            print("Packaged latest install, forced self-update, and pinned reinstall passed.", flush=True)
            if sys.platform == "darwin":
                partial = root / "missing-helper"
                env = installation_environment(partial, base_env)
                (partial / "bin").mkdir()
                extract_executable(archive, args.tag, args.target, "agentenv", Path(binary(partial)))
                with signing_keychain(partial, env, args.debug_keychain):
                    for operation in ["check", "set"]:
                        result = subprocess.run(
                            [binary(partial), "credential", operation, "fixture"],
                            env=env, cwd=partial, text=True, capture_output=True,
                            input="synthetic-unwritten-value\n" if operation == "set" else None,
                            timeout=30,
                        )
                        if result.returncode != 4 or "keychain setup" not in result.stderr:
                            raise RuntimeError(
                                f"Missing helper did not produce an actionable credential error: "
                                f"{result.returncode} {result.stdout} {result.stderr}"
                            )
                        if (partial / "data").exists():
                            raise RuntimeError("Credential access initialized missing helper state")
                print("Missing-helper read/write refusal passed.", flush=True)


if __name__ == "__main__":
    main()
