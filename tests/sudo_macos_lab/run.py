#!/usr/bin/env python3
"""Verify native macOS SSH against a disposable, localhost-only Linux fixture."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def server() -> int:
    sys.path.insert(0, "/opt/sudo-lab")
    from run_lab import RUNTIME, install_accounts, start_sshd

    RUNTIME.mkdir(mode=0o1777, exist_ok=True)
    RUNTIME.chmod(0o1777)
    install_accounts()
    process = start_sshd()
    process.terminate()
    process.communicate(timeout=5)
    config = RUNTIME / "sshd_config"
    config.write_text(config.read_text().replace("ListenAddress 127.0.0.1", "ListenAddress 0.0.0.0"))
    process = subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    signal.signal(signal.SIGTERM, lambda *_: process.terminate())
    print("fixture-ready", flush=True)
    return process.wait()


def main() -> int:
    if sys.argv[1:] == ["--server"]:
        return server()
    if len(sys.argv) == 4 and sys.argv[1] == "--provider":
        marker = Path(sys.argv[2]) / (sys.argv[3] + ".requests")
        with marker.open("a") as output:
            output.write("request\n")
        # Test-owned synthetic value matching the disposable account fixture.
        sys.stdout.write("SudoLab-Base-Only-42!")
        return 0
    if platform.system() != "Darwin":
        raise RuntimeError("this fixture requires a native macOS client")
    repo = Path(__file__).resolve().parents[2]
    app = repo / "target/debug/agentenv"
    if not all((app.parent / name).is_file() for name in ("agentenv", "agentenv-sudo-helper", "agentenv-ssh-askpass")):
        raise RuntimeError("build all three development binaries before running this fixture")

    def docker(*arguments: str) -> subprocess.CompletedProcess[bytes]:
        return subprocess.run(["docker", *arguments], cwd=repo, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=30)

    script = Path(__file__).resolve()
    container = docker("run", "--detach", "--rm", "--hostname", "agentenv-sudo-lab",
        "--add-host", "agentenv-sudo-lab:127.0.0.1", "--publish", "127.0.0.1::2222",
        "--mount", f"type=bind,source={script},target=/opt/sudo-lab/macos_fixture.py,readonly",
        "--entrypoint", "python3", "agentenv-sudo-execution:lab", "/opt/sudo-lab/macos_fixture.py", "--server").stdout.decode().strip()
    results: list[dict[str, object]] = []
    try:
        deadline = time.monotonic() + 10
        while b"fixture-ready" not in docker("logs", container).stdout:
            if time.monotonic() >= deadline:
                raise RuntimeError("disposable SSH fixture did not become ready")
            time.sleep(0.05)
        port = int(docker("port", container, "2222/tcp").stdout.decode().strip().rsplit(":", 1)[1])
        with tempfile.TemporaryDirectory(prefix="agentenv-macos-ssh-") as temporary:
            root = Path(temporary).resolve()
            public = docker("exec", container, "cat", "/etc/ssh/ssh_host_ed25519_key.pub").stdout.decode()
            known_hosts = root / "known_hosts"
            known_hosts.write_text("agentenv-macos-fixture " + public, encoding="ascii")
            identity = root / "identity"
            docker("cp", container + ":/run/sudo-lab/client_key", str(identity))
            identity.chmod(0o600)
            config = root / "agentenv.toml"

            def count(stage: str) -> int:
                marker = root / (stage + ".requests")
                return marker.read_text().count("request") if marker.exists() else 0

            for method in ("publickey", "password"):
                definitions = ""
                for stage, usage in (("login", "ssh-password"), ("sudo", "sudo")):
                    definitions += f'''[credentials.{stage}]
description = "Synthetic disposable {stage} credential."
provider = "command"
argv = {json.dumps([sys.executable, str(script), "--provider", str(root), stage])}
usages = ["{usage}"]
'''
                authentication = ('credential = "credential://login"' if method == "password" else
                                  f'identity_files = {json.dumps([str(identity)])}\nuse_agent = false')
                config.write_text(f'''version = 1
default_profile = "lab"
{definitions}
[profiles.lab]
description = "Disposable native SSH integration."
[profiles.lab.admin]
description = "Disposable remote account."
kind = "sudo-target"
[profiles.lab.admin.sudo]
transport = "ssh"
credential = "credential://sudo"
auth_user = "labuser"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.lab.admin.sudo.ssh]
mode = "explicit"
hostname = "127.0.0.1"
user = "labuser"
port = {port}
host_key_alias = "agentenv-macos-fixture"
known_hosts_file = {json.dumps(str(known_hosts))}
helper_path = "/opt/agentenv/agentenv-sudo-helper"
[profiles.lab.admin.sudo.ssh.auth]
method = "{method}"
{authentication}
''', encoding="utf-8")
                payload = bytes(range(256)) * 8193
                for name, command, data, sudo_count in (
                    ("check", ["--check", "--json"], b"", 0),
                    ("nopasswd", ["--", "/usr/bin/id", "-u"], b"", 0),
                    ("required", ["--", "/opt/sudo-lab/bin/allowed", "fixed"], b"", 1),
                    ("binary", ["--", "/opt/sudo-lab/bin/binary_probe"], payload, 1),
                ):
                    before = (count("login"), count("sudo"))
                    result = subprocess.run([str(app), "--no-project", "sudo", "--with", "admin",
                        "--connect-timeout-secs", "8", "--auth-timeout-secs", "8", *command], cwd=repo,
                        env={**os.environ, "AGENTENV_FILE": str(config)}, input=data,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
                    delta = (count("login") - before[0], count("sudo") - before[1])
                    valid = result.returncode == 0 and delta == (int(method == "password"), sudo_count)
                    if name == "nopasswd":
                        valid = valid and result.stdout == b"0\n"
                    elif name == "binary":
                        valid = valid and result.stdout == b"stdout:" + hashlib.sha256(data).hexdigest().encode() + b"\x00"
                        valid = valid and result.stderr == b"stderr:" + str(len(data)).encode() + b"\xff"
                    results.append({"name": method + "_" + name, "passed": valid,
                        "status": result.returncode, "lookups": delta})
    finally:
        docker("stop", "--time", "3", container)
    passed = all(result["passed"] for result in results)
    ssh_version = subprocess.run(["ssh", "-V"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, timeout=10).stdout.decode(errors="replace").strip()
    print(json.dumps({"passed": passed, "platform": platform.platform(), "ssh_version": ssh_version,
        "results": results}, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
