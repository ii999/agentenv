#!/usr/bin/env python3
"""Exercise built agentenv using synthetic credentials and disposable accounts."""

from __future__ import annotations

import concurrent.futures
import hashlib
import json
import os
import pwd
import signal
import subprocess
import time
from pathlib import Path

from run_lab import Lab, RUNTIME, install_accounts, checked


APP = "/opt/agentenv/agentenv"
CONFIG = Path("/run/sudo-lab/agentenv.toml")


def configure() -> None:
    CONFIG.write_text('''version = 1
default_profile = "lab"
[credentials.account]
description = "Synthetic lab password."
provider = "command"
argv = ["/usr/bin/python3", "/opt/sudo-lab/verify_agentenv.py", "--provider"]
usages = ["sudo"]
[profiles.lab]
description = "Disposable sudo lab."
[profiles.lab.admin]
description = "Local lab account."
kind = "sudo-target"
[profiles.lab.admin.sudo]
transport = "local"
credential = "credential://account"
auth_user = "labuser"
run_as = "root"
sudo_path = "/usr/bin/sudo"
''', encoding="utf-8")


def invoke(lab: Lab, command: list[str], data: bytes = b"", *, mode: str = "valid") -> subprocess.CompletedProcess[bytes]:
    return lab.command([APP, "--no-project", "sudo", "--with", "admin", "--", *command],
        user="labuser", stdin=data, env={"AGENTENV_FILE": str(CONFIG), "LAB_PROVIDER_MODE": mode}, timeout=20)


def cancellation_probe(lab: Lab) -> None:
    account = pwd.getpwnam("labuser")
    pid_file = RUNTIME / "cancel-target-pid"
    env = {**os.environ, "AGENTENV_FILE": str(CONFIG), "LAB_PROVIDER_MODE": "valid"}
    process = subprocess.Popen([
        APP, "--no-project", "sudo", "--with", "admin", "--",
        "/opt/sudo-lab/bin/long_running", str(pid_file),
    ], cwd="/tmp", env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        user=account.pw_uid, group=account.pw_gid, extra_groups=[account.pw_gid])
    target_pid = None
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and process.poll() is None:
            if pid_file.exists() and pid_file.read_text().strip():
                target_pid = int(pid_file.read_text())
                break
            time.sleep(0.02)
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=12)
        # A reported signal status must mean the target is gone; otherwise
        # only completion-unknown (10) is acceptable.
        gone = False
        deadline = time.monotonic() + 3
        while target_pid is not None and time.monotonic() < deadline and not gone:
            gone = not Path(f"/proc/{target_pid}").exists()
            time.sleep(0.05)
        lab.record("sigterm_is_forwarded_or_completion_is_explicitly_unknown",
            target_pid is not None and ((process.returncode == 143 and gone) or process.returncode == 10)
            and not stdout and b"SENTINEL" not in stderr,
            status=process.returncode, target_gone=gone)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        if target_pid is not None:
            try:
                # Disposable lab cleanup is root-owned, independently of the
                # unprivileged executor's observed termination capability.
                os.kill(target_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def main() -> int:
    import sys
    if sys.argv[1:] == ["--provider"]:
        # These bytes are test-owned. Only the resolver receives this stdout.
        from run_lab import BASE_PASSWORD
        with Path("/run/sudo-lab/provider-ran").open("a") as marker:
            marker.write("request\n")
        mode = os.environ.get("LAB_PROVIDER_MODE", "valid")
        if mode == "failure":
            sys.stderr.write("SENTINEL-PROVIDER-FAILURE")
            return 1
        value = {"valid": BASE_PASSWORD, "wrong": "SENTINEL-WRONG-PASSWORD", "newline": BASE_PASSWORD + "\n", "oversize": "X" * 256}[mode]
        sys.stdout.write(value)
        return 0
    RUNTIME.mkdir(mode=0o1777, exist_ok=True)
    RUNTIME.chmod(0o1777)
    install_accounts()
    retry_policy = Path("/etc/sudoers.d/z-agentenv")
    retry_policy.write_text("Defaults passwd_tries=3\nlabuser ALL=(root) PASSWD: /opt/sudo-lab/target.py *\n", encoding="ascii")
    retry_policy.chmod(0o440)
    checked(["visudo", "-cf", str(retry_policy)])
    configure()
    lab = Lab()
    marker = RUNTIME / "provider-ran"
    plan = lab.command([APP, "--no-project", "sudo", "--with", "admin", "--plan", "--json", "--", "/usr/bin/id", "-u"],
        user="labuser", env={"AGENTENV_FILE": str(CONFIG)})
    lab.record("plan_does_not_resolve", plan.returncode == 0 and not marker.exists())
    check = lab.command([APP, "--no-project", "sudo", "--with", "admin", "--check", "--json"],
        user="labuser", env={"AGENTENV_FILE": str(CONFIG)})
    lab.record("check_does_not_resolve", check.returncode == 0 and not marker.exists())
    passwordless = invoke(lab, ["/usr/bin/id", "-u"])
    lab.record("nopasswd_does_not_resolve", passwordless.returncode == 0 and passwordless.stdout == b"0\n" and not marker.exists(), status=passwordless.returncode)
    authenticated = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
    lab.record("real_password_and_exact_sudoers_rule", authenticated.returncode == 0 and marker.exists(), status=authenticated.returncode)
    forbidden = invoke(lab, ["/opt/sudo-lab/bin/allowed", "changed"])
    lab.record("sudoers_still_denies_changed_argv", forbidden.returncode != 0, status=forbidden.returncode)
    before = marker.read_text().count("request")
    wrong = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"], mode="wrong")
    lab.record("wrong_password_is_one_shot", wrong.returncode != 0 and marker.read_text().count("request") == before + 1 and b"SENTINEL" not in wrong.stdout + wrong.stderr, status=wrong.returncode)
    for mode in ["failure", "newline", "oversize"]:
        rejected = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"], mode=mode)
        lab.record("provider_" + mode + "_safe_rejection", rejected.returncode == 4 and b"SENTINEL" not in rejected.stdout + rejected.stderr and not rejected.stdout, status=rejected.returncode)
    payload = b"\n\x00\xff" + bytes(range(256)) * 2048
    binary = invoke(lab, ["/opt/sudo-lab/bin/binary_probe"], payload)
    expected_stdout = b"stdout:" + hashlib.sha256(payload).hexdigest().encode() + b"\x00"
    expected_stderr = b"stderr:" + str(len(payload)).encode() + b"\xff"
    lab.record("binary_stdin_separate_from_password", binary.returncode == 0 and binary.stdout == expected_stdout and binary.stderr == expected_stderr, status=binary.returncode, input_bytes=len(payload))
    arguments = ["", "white space", "é日本", "'\"quotes", "$(not-a-command);", "line\nbreak", "--leading"]
    for status in [0, 1, 9, 10, 127, 255]:
        result = invoke(lab, ["/opt/sudo-lab/target.py", str(status), *arguments])
        lab.record("literal_argv_binary_output_and_status_" + str(status),
            result.returncode == status
            and result.stdout == json.dumps(arguments, ensure_ascii=False).encode() + b"\x00\xff"
            and result.stderr == b"target-stderr\x00\xfe", status=result.returncode)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        statuses = list(pool.map(lambda _: invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"]).returncode, range(4)))
    lab.record("concurrent_invocations", statuses == [0] * 4, statuses=statuses)
    cancellation_probe(lab)
    report = {"passed": all(item["passed"] for item in lab.results), "results": lab.results}
    # Emit fixed statuses, never raw app/provider output or candidate bytes.
    print(json.dumps(report, sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
