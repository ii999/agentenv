#!/usr/bin/env python3
"""Run only inside the disposable lab: release-matrix sudo policy and SSH refusal cases."""

from __future__ import annotations

import hashlib
import json
import os
import pwd
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, "/opt/sudo-lab")

from run_lab import BASE_PASSWORD, Lab, RUNTIME, checked, install_accounts, start_sshd  # noqa: E402
import verify_agentenv as local  # noqa: E402
import verify_ssh as remote  # noqa: E402

SECRET = BASE_PASSWORD.encode()
LIBEXEC = Path("/usr/local/libexec/agentenv-release")
POLICY = Path("/etc/sudoers.d/z-release")
IOLOG = Path("/var/log/agentenv-release-io")
ROUTING = (b"AGENTENV_SUDO_SOCKET", b"AGENTENV_SUDO_SESSION", b"AGENTENV_SSH_SOCKET", b"AGENTENV_SSH_SESSION")


def install_probes() -> None:
    LIBEXEC.mkdir(parents=True, mode=0o755, exist_ok=True)
    probes = {
        # Reports the complete target environment; the verifier inspects it.
        "env_probe": "import os, sys\nsys.stdout.buffer.write(b'\\0'.join(k + b'=' + v for k, v in os.environb.items()))\n",
        "self_kill": "import os, signal\nos.kill(os.getpid(), signal.SIGKILL)\n",
    }
    for name, body in probes.items():
        path = LIBEXEC / name
        path.write_text("#!/usr/bin/python3\n" + body, encoding="ascii")
        path.chmod(0o755)


def policy(*defaults: str) -> None:
    """Replaces the release policy with fixed rules plus the given defaults."""
    rules = [f"Defaults:labuser {value}" for value in defaults]
    rules += [
        f"labuser ALL=(root) PASSWD: {LIBEXEC}/env_probe, {LIBEXEC}/self_kill, {LIBEXEC}/missing",
    ]
    POLICY.write_text("\n".join(rules) + "\n", encoding="ascii")
    POLICY.chmod(0o440)
    checked(["visudo", "-cf", str(POLICY)])


def lookups() -> int:
    marker = RUNTIME / "provider-ran"
    return marker.read_text().count("request") if marker.exists() else 0


def run_local(lab: Lab, command: list[str], data: bytes = b"") -> tuple[subprocess.CompletedProcess[bytes], int]:
    before = lookups()
    result = local.invoke(lab, command, data)
    return result, lookups() - before


def local_cases(lab: Lab) -> None:
    local.configure()
    policy()
    result, count = run_local(lab, [f"{LIBEXEC}/env_probe"])
    variables = result.stdout.split(b"\0")
    default_names = {v.split(b"=", 1)[0] for v in variables}
    lab.record("default_env_policy_target_environment_has_no_password",
        result.returncode == 0 and count == 1 and variables != [b""] and SECRET not in result.stdout,
        status=result.returncode, lookups=count,
        routing_metadata_visible=any(v.startswith(name + b"=") for v in variables for name in ROUTING))

    policy("!env_reset")
    result, count = run_local(lab, [f"{LIBEXEC}/env_probe"])
    variables = result.stdout.split(b"\0")
    preserved_names = {v.split(b"=", 1)[0] for v in variables}
    lab.record("preserved_env_policy_target_environment_has_no_password",
        result.returncode == 0 and count == 1 and variables != [b""]
        and bool(preserved_names - default_names) and SECRET not in result.stdout,
        status=result.returncode, lookups=count,
        routing_metadata_visible=any(v.startswith(name + b"=") for v in variables for name in ROUTING))

    if IOLOG.exists():
        shutil.rmtree(IOLOG)
    policy("log_input", "log_output", "!compress_io", f"iolog_dir={IOLOG}")
    marker = b"agentenv-release-stdin-marker"
    result, count = run_local(lab, ["/opt/sudo-lab/bin/binary_probe"], marker)
    logged = b"".join(path.read_bytes() for path in IOLOG.rglob("*") if path.is_file()) if IOLOG.exists() else b""
    lab.record("sudo_io_log_records_input_but_not_password",
        result.returncode == 0 and count == 1 and marker in logged and SECRET not in logged,
        status=result.returncode, lookups=count, logged_bytes=len(logged))

    policy("!use_pty")
    payload = b"\n\x00\xff" + bytes(range(256)) * 512
    before = lookups()
    result = local.invoke(lab, ["/opt/sudo-lab/bin/binary_probe"], payload)
    lab.record("without_use_pty_binary_streams_and_status_are_preserved",
        result.returncode == 0
        and result.stdout == b"stdout:" + hashlib.sha256(payload).hexdigest().encode() + b"\x00"
        and result.stderr == b"stderr:" + str(len(payload)).encode() + b"\xff",
        status=result.returncode, lookups=lookups() - before)

    policy("requiretty")
    result, count = run_local(lab, ["/opt/sudo-lab/bin/binary_probe"])
    lab.record("requiretty_is_rejected_without_password_or_pty_fallback",
        result.returncode != 0 and count == 0 and not result.stdout and SECRET not in result.stderr,
        status=result.returncode, lookups=count)

    for option in ("rootpw", "targetpw", "runaspw"):
        policy(option)
        result, count = run_local(lab, [f"{LIBEXEC}/env_probe"])
        lab.record(option + "_other_account_prompt_releases_no_password",
            result.returncode != 0 and count == 0 and not result.stdout,
            status=result.returncode, lookups=count)

    policy()
    result, count = run_local(lab, [f"{LIBEXEC}/self_kill"])
    lab.record("target_signal_termination_is_reported", result.returncode == 128 + 9,
        status=result.returncode, lookups=count)
    result, count = run_local(lab, [f"{LIBEXEC}/missing"])
    lab.record("missing_executable_fails_without_output", result.returncode != 0 and not result.stdout,
        status=result.returncode, lookups=count)
    POLICY.unlink()


def extra_sshd(port: int, password: bool, keyboard: bool) -> subprocess.Popen[bytes]:
    config = RUNTIME / f"sshd_config_{port}"
    text = (RUNTIME / "sshd_config").read_text()
    text = text.replace("Port 2222", f"Port {port}").replace("PidFile /run/sudo-lab/sshd.pid", f"PidFile /run/sudo-lab/sshd-{port}.pid")
    text = text.replace("PasswordAuthentication yes", f"PasswordAuthentication {'yes' if password else 'no'}")
    text = text.replace("KbdInteractiveAuthentication no", f"KbdInteractiveAuthentication {'yes' if keyboard else 'no'}")
    config.write_text(text)
    process = subprocess.Popen(["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return process
        time.sleep(0.05)
    process.terminate()
    raise RuntimeError("fixture sshd did not start")


def configure_remote(port: int) -> None:
    remote.configure("password")
    remote.CONFIG.write_text(remote.CONFIG.read_text().replace("port = 2222", f"port = {port}"), encoding="utf-8")


def remote_record(lab: Lab, name: str, result: subprocess.CompletedProcess[bytes], before: tuple[int, int],
                  expected: tuple[int, int]) -> None:
    delta = tuple(a - b for a, b in zip(remote.counts(), before))
    lab.record(name, result.returncode != 0 and delta == expected and not result.stdout
        and b"SENTINEL" not in result.stderr and SECRET not in result.stderr,
        status=result.returncode, lookups=delta)


def remote_cases(lab: Lab) -> None:
    servers = [start_sshd(), extra_sshd(2224, password=False, keyboard=False),
               extra_sshd(2225, password=False, keyboard=True)]
    account = pwd.getpwnam("labuser")
    os.chown(RUNTIME / "client_key", account.pw_uid, account.pw_gid)
    host_key = Path("/etc/ssh/ssh_host_ed25519_key.pub").read_text()
    remote.KNOWN_HOSTS.write_text("agentenv-lab " + host_key, encoding="ascii")
    try:
        for port, name in ((2224, "password_method_disabled_by_server"), (2225, "keyboard_interactive_only_server")):
            configure_remote(port)
            before = remote.counts()
            result = remote.invoke(lab, ["/usr/bin/id", "-u"])
            remote_record(lab, name + "_releases_no_password", result, before, (0, 0))

        # The executable is copied without its local SSH askpass companion.
        partial = Path("/tmp/agentenv-without-askpass")
        partial.mkdir(mode=0o755, exist_ok=True)
        for name in ("agentenv", "agentenv-sudo-helper"):
            shutil.copy2("/opt/agentenv/" + name, partial / name)
        configure_remote(2222)
        before = remote.counts()
        result = lab.command([str(partial / "agentenv"), *remote.invocation(["/usr/bin/id", "-u"])[1:]],
            user="labuser", env={"AGENTENV_FILE": str(remote.CONFIG)}, timeout=30)
        remote_record(lab, "missing_local_askpass_companion_fails_before_lookup", result, before, (0, 0))

        # Cancelling a passwordless remote target must still return the
        # observed signal status, with the target gone.
        pid_file = RUNTIME / "nopasswd-cancel-pid"
        rule = Path("/etc/sudoers.d/z-release-remote")
        rule.write_text(f"labuser ALL=(root) NOPASSWD: /opt/sudo-lab/bin/long_running {pid_file}\n", encoding="ascii")
        rule.chmod(0o440)
        checked(["visudo", "-cf", str(rule)])
        remote.configure()
        before = remote.counts()
        process = subprocess.Popen(remote.invocation(["/opt/sudo-lab/bin/long_running", str(pid_file)]),
            cwd="/tmp", env={**os.environ, "AGENTENV_FILE": str(remote.CONFIG)}, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, user=account.pw_uid, group=account.pw_gid,
            extra_groups=[account.pw_gid])
        target = None
        try:
            deadline = time.monotonic() + 12
            while time.monotonic() < deadline and process.poll() is None and target is None:
                if pid_file.exists() and pid_file.read_text().strip():
                    target = int(pid_file.read_text())
                time.sleep(0.02)
            process.send_signal(signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=15)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)
        delta = tuple(a - b for a, b in zip(remote.counts(), before))
        lab.record("nopasswd_remote_cancel_reports_observed_signal",
            target is not None and process.returncode == 128 + signal.SIGTERM and remote.target_gone(target)
            and delta == (0, 0) and not stdout and SECRET not in stderr,
            status=process.returncode, lookups=delta)
        rule.unlink()

        # A login-time script writing to the session stdout precedes the
        # helper's Ready frame and must fail before any sudo lookup.
        rc = Path(account.pw_dir) / ".ssh" / "rc"
        rc.write_text("echo agentenv-release-banner\n", encoding="ascii")
        os.chown(rc, account.pw_uid, account.pw_gid)
        remote.configure()
        before = remote.counts()
        result = remote.invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
        remote_record(lab, "startup_banner_on_protocol_stdout_releases_no_password", result, before, (0, 0))
        rc.unlink()

        checked(["useradd", "--create-home", "--shell", "/bin/sh", "expired"])
        checked(["chpasswd"], stdin=f"expired:{BASE_PASSWORD}\n".encode())
        checked(["chage", "-d", "0", "expired"])
        remote.configure("password")
        remote.CONFIG.write_text(remote.CONFIG.read_text().replace('"labuser"', '"expired"'), encoding="utf-8")
        before = remote.counts()
        result = remote.invoke(lab, ["/usr/bin/id", "-u"])
        delta = tuple(a - b for a, b in zip(remote.counts(), before))
        lab.record("expired_password_change_is_not_answered",
            result.returncode != 0 and delta[1] == 0 and delta[0] <= 1 and not result.stdout
            and SECRET not in result.stderr, status=result.returncode, lookups=delta)
    finally:
        for process in servers:
            process.terminate()
            process.wait(timeout=5)


def main() -> int:
    RUNTIME.mkdir(mode=0o1777, exist_ok=True)
    RUNTIME.chmod(0o1777)
    install_accounts()
    install_probes()
    lab = Lab()
    local_cases(lab)
    remote_cases(lab)
    report = {"passed": all(item["passed"] for item in lab.results), "results": lab.results}
    print(json.dumps(report, sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
