#!/usr/bin/env python3
"""Run only inside the disposable lab: real SSH and sudo with synthetic secrets."""

from __future__ import annotations

import concurrent.futures
import hashlib
import json
import os
import pwd
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

from run_lab import BASE_PASSWORD, Lab, RUNTIME, checked, install_accounts, start_sshd

APP = "/opt/agentenv/agentenv"
CONFIG = RUNTIME / "ssh-agentenv.toml"
KNOWN_HOSTS = RUNTIME / "agentenv-known-hosts"
AGENT_SOCKET = RUNTIME / "test-agent.sock"


def provider(stage: str) -> int:
    with (RUNTIME / (stage + "-provider-ran")).open("a") as marker:
        marker.write("request\n")
    mode = os.environ.get("LAB_" + stage.upper() + "_MODE", "valid")
    if mode == "slow":
        time.sleep(2)
        mode = "valid"
    if mode == "failure":
        sys.stderr.write("SENTINEL-PROVIDER-FAILURE")
        return 1
    value = {"valid": BASE_PASSWORD, "wrong": "SENTINEL-WRONG", "newline": BASE_PASSWORD + "\n", "oversize": "X" * 256}[mode]
    sys.stdout.write(value)
    return 0


def counts() -> tuple[int, int]:
    return tuple((RUNTIME / (stage + "-provider-ran")).read_text().count("request")
                 if (RUNTIME / (stage + "-provider-ran")).exists() else 0
                 for stage in ("login", "sudo"))


def configure(auth: str = "publickey", *, known_hosts: Path = KNOWN_HOSTS,
              helper: str = "/opt/agentenv/agentenv-sudo-helper", shared: bool = False,
              use_agent: bool = False, files: bool = True) -> None:
    definitions = ""
    for stage in ("login", "sudo"):
        usages = ["ssh-password", "sudo"] if shared and stage == "login" else ["ssh-password" if stage == "login" else "sudo"]
        definitions += f'''[credentials.{stage}]
description = "Synthetic {stage} credential."
provider = "command"
argv = ["/usr/bin/python3", "/opt/sudo-lab/verify_ssh.py", "--provider", "{stage}"]
usages = {json.dumps(usages)}
'''
    auth_config = ('credential = "credential://login"' if auth == "password" else
                   f'identity_files = {json.dumps([str(RUNTIME / "client_key")] if files else [])}\nuse_agent = {str(use_agent).lower()}')
    CONFIG.write_text(f'''version = 1
default_profile = "lab"
{definitions}
[profiles.lab]
description = "Disposable remote execution lab."
[profiles.lab.admin]
description = "Loopback SSH test account."
kind = "sudo-target"
[profiles.lab.admin.sudo]
transport = "ssh"
credential = "credential://{'login' if shared else 'sudo'}"
auth_user = "labuser"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.lab.admin.sudo.ssh]
host_key_alias = "agentenv-lab"
known_hosts_file = "{known_hosts}"
helper_path = "{helper}"
mode = "explicit"
hostname = "127.0.0.1"
user = "labuser"
port = 2222
[profiles.lab.admin.sudo.ssh.auth]
method = "{auth}"
{auth_config}
''', encoding="utf-8")


def invocation(command: list[str], *, check: bool = False, setup_timeout: int = 8, auth_timeout: int = 8) -> list[str]:
    base = [APP, "--no-project", "sudo", "--with", "admin", "--connect-timeout-secs", str(setup_timeout), "--auth-timeout-secs", str(auth_timeout)]
    return base + (["--check", "--json"] if check else ["--", *command])


def invoke(lab: Lab, command: list[str] | None = None, data: bytes = b"", *, check: bool = False,
           login_mode: str = "valid", sudo_mode: str = "valid", setup_timeout: int = 8,
           auth_timeout: int = 8) -> subprocess.CompletedProcess[bytes]:
    return lab.command(invocation(command or [], check=check, setup_timeout=setup_timeout, auth_timeout=auth_timeout), user="labuser", stdin=data,
        env={"AGENTENV_FILE": str(CONFIG), "LAB_LOGIN_MODE": login_mode, "LAB_SUDO_MODE": sudo_mode,
             "SSH_AUTH_SOCK": str(AGENT_SOCKET)}, timeout=30)


def record(lab: Lab, name: str, result: subprocess.CompletedProcess[bytes], before: tuple[int, int],
           expected: tuple[int, int], valid: bool) -> None:
    after = counts()
    delta = tuple(a - b for a, b in zip(after, before))
    lab.record(name, valid and delta == expected and b"SENTINEL" not in result.stdout + result.stderr,
        status=result.returncode, lookups=delta)


def target_gone(pid: int | None) -> bool:
    deadline = time.monotonic() + 3
    while pid is not None and time.monotonic() < deadline:
        if not Path(f"/proc/{pid}").exists():
            return True
        time.sleep(0.05)
    return False


def lifecycle(lab: Lab, *, disconnect: bool, group: bool = False) -> None:
    """Signals the CLI (or its whole process group, as a terminal or harness
    would), or kills ssh to simulate a lost connection."""
    configure()
    account = pwd.getpwnam("labuser")
    pid_file = RUNTIME / ("ssh-disconnect-pid" if disconnect else "ssh-group-pid" if group else "ssh-cancel-pid")
    process = subprocess.Popen(invocation(["/opt/sudo-lab/bin/long_running", str(pid_file)]),
        cwd="/tmp", env={**os.environ, "AGENTENV_FILE": str(CONFIG)}, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, user=account.pw_uid, group=account.pw_gid,
        extra_groups=[account.pw_gid], start_new_session=group)
    target_pid = None
    try:
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline and process.poll() is None:
            if pid_file.exists() and pid_file.read_text().strip():
                target_pid = int(pid_file.read_text())
                break
            time.sleep(0.02)
        if disconnect and target_pid is not None:
            children = Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()
            for child in children:
                if Path(f"/proc/{child}/comm").read_text().strip() == "ssh":
                    os.kill(int(child), signal.SIGKILL)
        elif group:
            os.killpg(process.pid, signal.SIGINT)
        else:
            process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=15)
        if disconnect:
            name, valid = "disconnect_reports_unknown_without_retry", process.returncode == 10
        else:
            # An observed signal status is valid only if the target is gone.
            expected = 128 + (signal.SIGINT if group else signal.SIGTERM)
            name = "remote_group_signal_completion" if group else "remote_signal_completion"
            valid = process.returncode == expected and target_gone(target_pid)
        lab.record(name, target_pid is not None and valid and not stdout and b"SENTINEL" not in stderr,
            status=process.returncode)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        if target_pid is not None:
            try:
                os.kill(target_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def agent_authentication(lab: Lab) -> None:
    account = pwd.getpwnam("labuser")
    agent = subprocess.Popen(["/usr/bin/ssh-agent", "-D", "-a", str(AGENT_SOCKET)],
        cwd="/tmp", stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        user=account.pw_uid, group=account.pw_gid, extra_groups=[account.pw_gid])
    try:
        deadline = time.monotonic() + 5
        while not AGENT_SOCKET.exists() and time.monotonic() < deadline and agent.poll() is None:
            time.sleep(0.02)
        added = lab.command(["/usr/bin/ssh-add", str(RUNTIME / "client_key")], user="labuser",
            env={"SSH_AUTH_SOCK": str(AGENT_SOCKET)})
        if added.returncode != 0:
            raise RuntimeError("synthetic SSH agent could not load its test key")
        for files, use_agent in ((True, False), (True, True), (False, True), (False, False)):
            configure(files=files, use_agent=use_agent)
            before = counts()
            result = invoke(lab, ["/usr/bin/id", "-u"])
            valid = result.returncode == (0 if files or use_agent else 2)
            if files or use_agent:
                valid = valid and result.stdout == b"0\n"
            record(lab, f"selected_keys_files_{files}_agent_{use_agent}", result, before, (0, 0), valid)
    finally:
        agent.terminate()
        agent.wait(timeout=5)


def jump_routes(lab: Lab) -> None:
    servers: list[subprocess.Popen[bytes]] = []
    known = RUNTIME / "jump-known-hosts"
    known.write_text("", encoding="ascii")
    try:
        for index, port in ((1, 2223), (2, 2224)):
            key = RUNTIME / f"jump-host-key-{index}"
            checked(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)])
            with known.open("a") as output:
                output.write(f"jump-trust-{index} " + key.with_suffix(".pub").read_text())
            configuration = RUNTIME / f"jump-sshd-{index}"
            configuration.write_text((RUNTIME / "sshd_config").read_text()
                .replace("Port 2222", f"Port {port}")
                .replace("/etc/ssh/ssh_host_ed25519_key", str(key))
                .replace("/run/sudo-lab/sshd.pid", f"/run/sudo-lab/jump-{index}.pid"))
            process = subprocess.Popen(["/usr/sbin/sshd", "-D", "-f", str(configuration)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            servers.append(process)
            deadline = time.monotonic() + 5
            while True:
                with socket.socket() as probe:
                    if probe.connect_ex(("127.0.0.1", port)) == 0:
                        break
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("synthetic jump server failed to start")
                time.sleep(0.02)
        ssh_config = RUNTIME / "agentenv-ssh-config"
        policy = f'''Host destination
    HostName 127.0.0.1
    Port 2222
    ProxyJump hop1,hop2
Host hop1
    HostName 127.0.0.1
    Port 2223
    HostKeyAlias jump-trust-1
Host hop2
    HostName 127.0.0.1
    Port 2224
    HostKeyAlias jump-trust-2
Host *
    User labuser
    BatchMode yes
    PreferredAuthentications publickey
    PubkeyAuthentication yes
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    GSSAPIAuthentication no
    HostbasedAuthentication no
    IdentityFile /run/sudo-lab/client_key
    IdentitiesOnly yes
    IdentityAgent none
    StrictHostKeyChecking yes
    UserKnownHostsFile {known}
    GlobalKnownHostsFile none
    VerifyHostKeyDNS no
    KnownHostsCommand none
    UpdateHostKeys no
    ControlMaster no
    ControlPath none
    ControlPersist no
    ForwardAgent no
    ForwardX11 no
    ForwardX11Trusted no
    PermitLocalCommand no
    ClearAllForwardings yes
    SecurityKeyProvider none
    PKCS11Provider none
'''

        def alias_target(method: str = "publickey") -> None:
            configure(method)
            text = CONFIG.read_text().replace(
                'mode = "explicit"\nhostname = "127.0.0.1"\nuser = "labuser"\nport = 2222',
                f'mode = "ssh-config"\nhost_alias = "destination"\nconfig_file = "{ssh_config}"')
            text = text.replace('identity_files = ["/run/sudo-lab/client_key"]\nuse_agent = false\n', "")
            CONFIG.write_text(text)

        for name, text, method, expected, status in (
            ("valid_native_two_hop_route", policy, "publickey", (0, 1), 0),
            ("unsafe_inner_auth_rejected", policy.replace("Host hop1\n", "Host hop1\n    PasswordAuthentication yes\n"), "publickey", (0, 0), 9),
            ("duplicate_final_trust_alias_rejected", policy.replace("HostKeyAlias jump-trust-1", "HostKeyAlias agentenv-lab"), "publickey", (0, 0), 9),
            ("jump_password_mode_rejected", policy, "password", (0, 0), 9),
            ("effective_alias_user_mismatch_rejected", policy.replace("Host destination\n", "Host destination\n    User nobody\n"), "publickey", (0, 0), 9),
        ):
            ssh_config.write_text(text)
            alias_target(method)
            before = counts()
            result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
            record(lab, name, result, before, expected, result.returncode == status)
    finally:
        for process in servers:
            process.terminate()
            process.wait(timeout=5)


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "--provider":
        return provider(sys.argv[2])
    RUNTIME.mkdir(mode=0o1777, exist_ok=True)
    RUNTIME.chmod(0o1777)
    install_accounts()
    policy = Path("/etc/sudoers.d/z-agentenv")
    policy.write_text("Defaults passwd_tries=3\nlabuser ALL=(root) PASSWD: /opt/sudo-lab/target.py *\n", encoding="ascii")
    policy.chmod(0o440)
    checked(["visudo", "-cf", str(policy)])
    server = start_sshd()
    account = pwd.getpwnam("labuser")
    os.chown(RUNTIME / "client_key", account.pw_uid, account.pw_gid)
    KNOWN_HOSTS.write_text("agentenv-lab " + Path("/etc/ssh/ssh_host_ed25519_key.pub").read_text(), encoding="ascii")
    lab = Lab()
    try:
        agent_authentication(lab)
        jump_routes(lab)
        for auth in ("publickey", "password"):
            configure(auth)
            for check, command, sudo_count in [(True, [], 0), (False, ["/usr/bin/id", "-u"], 0), (False, ["/opt/sudo-lab/bin/allowed", "fixed"], 1)]:
                before = counts()
                result = invoke(lab, command, check=check)
                valid = result.returncode == 0 and (check or command[0] != "/usr/bin/id" or result.stdout == b"0\n")
                record(lab, auth + ("_check" if check else "_sudo_" + str(sudo_count)), result, before, (int(auth == "password"), sudo_count), valid)
        configure("password", shared=True)
        before = counts()
        result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
        record(lab, "shared_reference_resolves_once_per_stage", result, before, (2, 0), result.returncode == 0)
        configure("password")
        before = counts()
        result = invoke(lab, ["/usr/bin/id", "-u"], login_mode="slow", setup_timeout=1, auth_timeout=4)
        record(lab, "login_authentication_excluded_from_setup_budget", result, before, (1, 0), result.returncode == 0 and result.stdout == b"0\n")
        before = counts()
        result = invoke(lab, ["/usr/bin/id", "-u"], login_mode="slow", auth_timeout=1)
        record(lab, "login_resolution_authentication_deadline", result, before, (1, 0), result.returncode == 4 and not result.stdout)
        for mode in ("wrong", "failure", "newline", "oversize"):
            before = counts()
            result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"], login_mode=mode)
            record(lab, "login_" + mode + "_one_shot", result, before, (1, 0), result.returncode == (9 if mode == "wrong" else 4))
        before = counts()
        result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"], sudo_mode="wrong")
        record(lab, "wrong_sudo_no_cross_stage_retry", result, before, (1, 1), result.returncode != 0)
        unknown = RUNTIME / "empty-known-hosts"
        unknown.write_text("", encoding="ascii")
        configure("password", known_hosts=unknown)
        before = counts()
        result = invoke(lab, ["/usr/bin/id", "-u"])
        record(lab, "unknown_host_key_before_credentials", result, before, (0, 0), result.returncode == 9)
        configure(helper="/opt/agentenv/missing-helper")
        before = counts()
        result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
        record(lab, "missing_helper_before_sudo_credential", result, before, (0, 0), result.returncode == 9)
        configure()
        payload = b"\n\x00\xff" + bytes(range(256)) * 8193
        result = invoke(lab, ["/opt/sudo-lab/bin/binary_probe"], payload)
        lab.record("binary_stdin_over_two_mib", result.returncode == 0
            and result.stdout == b"stdout:" + hashlib.sha256(payload).hexdigest().encode() + b"\x00"
            and result.stderr == b"stderr:" + str(len(payload)).encode() + b"\xff", status=result.returncode)
        result = invoke(lab, ["/opt/sudo-lab/target.py", "--bulk"])
        lab.record("independent_binary_output_over_two_mib_each", result.returncode == 0
            and result.stdout == bytes(range(256)) * 8193 and result.stderr == bytes(reversed(range(256))) * 8193,
            status=result.returncode, stdout_bytes=len(result.stdout), stderr_bytes=len(result.stderr))
        arguments = ["", "white space", "é日本", "'\"quotes", "$(not-a-command);", "line\nbreak", "--leading"]
        for status in (0, 1, 9, 10, 127, 255):
            result = invoke(lab, ["/opt/sudo-lab/target.py", str(status), *arguments], payload)
            lab.record("literal_argv_early_exit_status_" + str(status), result.returncode == status
                and result.stdout == json.dumps(arguments, ensure_ascii=False).encode() + b"\x00\xff"
                and result.stderr == b"target-stderr\x00\xfe", status=result.returncode)
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            statuses = list(pool.map(lambda _: invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"]).returncode, range(4)))
        lab.record("concurrent_remote_invocations", statuses == [0] * 4, statuses=statuses)
        lifecycle(lab, disconnect=False)
        lifecycle(lab, disconnect=False, group=True)
        lifecycle(lab, disconnect=True)
    finally:
        server.terminate()
        server.communicate(timeout=5)
    passed = all(item["passed"] for item in lab.results)
    print(json.dumps({"passed": passed, "results": lab.results}, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
