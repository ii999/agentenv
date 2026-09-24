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


DEPLOY_DIR = RUNTIME / "deploy"
DEPLOYED_HELPER = DEPLOY_DIR / "libexec" / "agentenv-sudo-helper"
BUNDLED_HELPER = "/opt/agentenv/agentenv-sudo-helper"


def deploy(lab: Lab, *extra: str, login_mode: str = "valid") -> subprocess.CompletedProcess[bytes]:
    argv = [APP, "--no-project", "--json", "sudo", "--with", "admin", "--connect-timeout-secs", "8",
            "--auth-timeout-secs", "8", "--deploy-helper", *extra]
    return lab.command(argv, user="labuser", env={"AGENTENV_FILE": str(CONFIG), "LAB_LOGIN_MODE": login_mode,
        "LAB_SUDO_MODE": "valid", "SSH_AUTH_SOCK": str(AGENT_SOCKET), "HOME": "/home/labuser"}, timeout=60)


def report(result: subprocess.CompletedProcess[bytes]) -> dict:
    try:
        return json.loads(result.stdout)
    except ValueError:
        return {}


def failure(result: subprocess.CompletedProcess[bytes]) -> str:
    """The failure line agentenv printed, for the record of a failed case."""
    return result.stderr.decode(errors="replace").strip()[-400:]


def mtime_ns(path: Path) -> int | None:
    return path.stat().st_mtime_ns if path.exists() else None


def temporaries() -> list[Path]:
    """Upload temporaries beside the deployed helper, with any pid suffix."""
    return sorted(DEPLOYED_HELPER.parent.glob(DEPLOYED_HELPER.name + ".agentenv-new*"))


def helper_identity() -> str:
    return subprocess.run([BUNDLED_HELPER, "--identity"], capture_output=True, text=True, check=True).stdout.strip()


def deployment(lab: Lab) -> None:
    """Explicit helper deployment through the same SSH route as execution."""
    import shutil
    import stat

    shutil.rmtree(DEPLOY_DIR, ignore_errors=True)
    account = pwd.getpwnam("labuser")
    identity = helper_identity()
    configure(helper=str(DEPLOYED_HELPER))

    # A first installation from the bundled companion (same target as the
    # destination): 0755, user-owned, identity verified, check handshake done.
    before = counts()
    result = deploy(lab)
    data = report(result)
    mode = DEPLOYED_HELPER.stat().st_mode if DEPLOYED_HELPER.exists() else 0
    lab.record("deploy_first_install_from_bundle", result.returncode == 0 and data.get("status") == "deployed"
        and data.get("previous") is None and data.get("installed") == identity
        and data.get("source", {}).get("kind") == "bundle" and data.get("helper", {}).get("build")
        and stat.S_IMODE(mode) == 0o755 and DEPLOYED_HELPER.stat().st_uid == account.pw_uid
        and not temporaries()
        and counts() == before, status=result.returncode, report={k: data.get(k) for k in ("status", "previous", "source")},
        failure=failure(result))

    # The deployed helper serves real remote sudo execution.
    before = counts()
    result = invoke(lab, ["/opt/sudo-lab/bin/allowed", "fixed"])
    record(lab, "deploy_then_execute_through_deployed_helper", result, before, (0, 1), result.returncode == 0)

    # A rerun changes nothing; --force reinstalls the same identity.
    mtime = mtime_ns(DEPLOYED_HELPER)
    result = deploy(lab)
    data = report(result)
    lab.record("deploy_rerun_is_up_to_date", result.returncode == 0 and data.get("status") == "up-to-date"
        and data.get("source") is None and mtime_ns(DEPLOYED_HELPER) == mtime, status=result.returncode,
        failure=failure(result))
    result = deploy(lab, "--force")
    data = report(result)
    lab.record("deploy_force_reinstalls", result.returncode == 0 and data.get("status") == "deployed"
        and data.get("previous") == identity, status=result.returncode, failure=failure(result))

    # An older helper at the path is upgraded, and --from takes the given file.
    DEPLOYED_HELPER.parent.mkdir(parents=True, exist_ok=True)
    DEPLOYED_HELPER.write_text("#!/bin/sh\ncase \"$1\" in --identity) echo 'agentenv-sudo-helper 1 0.0.1';; *) exit 9;; esac\n")
    DEPLOYED_HELPER.chmod(0o755)
    os.chown(DEPLOYED_HELPER, account.pw_uid, account.pw_gid)
    result = deploy(lab, "--from", BUNDLED_HELPER)
    data = report(result)
    lab.record("deploy_upgrades_older_helper_from_file", result.returncode == 0 and data.get("status") == "deployed"
        and data.get("previous") == "agentenv-sudo-helper 1 0.0.1" and data.get("source", {}).get("kind") == "file"
        and DEPLOYED_HELPER.read_bytes() == Path(BUNDLED_HELPER).read_bytes(), status=result.returncode,
        failure=failure(result))

    # Refusals leave the destination untouched: a wrong source (forced past
    # the up-to-date decision so it is actually uploaded and verified), an
    # occupied path, and a login shell that prints on stdout.
    result = deploy(lab, "--force", "--from", "/opt/sudo-lab/target.py")
    lab.record("deploy_refuses_wrong_source", result.returncode == 9 and b"helper-deploy-identity-mismatch" in result.stderr
        and DEPLOYED_HELPER.exists() and DEPLOYED_HELPER.read_bytes() == Path(BUNDLED_HELPER).read_bytes()
        and not temporaries(), status=result.returncode,
        failure=failure(result))
    DEPLOYED_HELPER.unlink(missing_ok=True)
    DEPLOYED_HELPER.write_bytes(b"not a helper")
    os.chown(DEPLOYED_HELPER, account.pw_uid, account.pw_gid)
    result = deploy(lab)
    lab.record("deploy_refuses_occupied_path", result.returncode == 9 and b"helper-deploy-path-occupied" in result.stderr
        and DEPLOYED_HELPER.read_bytes() == b"not a helper", status=result.returncode, failure=failure(result))
    DEPLOYED_HELPER.unlink()
    checked(["usermod", "--shell", "/bin/bash", "labuser"])
    bashrc = Path("/home/labuser/.bashrc")
    previous_rc = bashrc.read_bytes() if bashrc.exists() else None
    bashrc.write_text("echo 'Welcome to the lab'\n")
    try:
        result = deploy(lab)
        libexec = DEPLOY_DIR / "libexec"
        untouched = not DEPLOYED_HELPER.exists() and (not libexec.exists() or not any(libexec.iterdir()))
        lab.record("deploy_refuses_stdout_banner_before_writing", result.returncode == 9
            and b"helper-deploy-preflight-unparseable" in result.stderr and untouched, status=result.returncode,
            failure=failure(result))
    finally:
        if previous_rc is None:
            bashrc.unlink()
        else:
            bashrc.write_bytes(previous_rc)
        checked(["usermod", "--shell", "/bin/sh", "labuser"])

    # Saved-password login: three sessions, three login replies, no sudo lookup.
    configure("password", helper=str(DEPLOYED_HELPER))
    before = counts()
    result = deploy(lab)
    data = report(result)
    after = counts()
    lab.record("deploy_with_password_login_uses_three_sessions", result.returncode == 0 and data.get("status") == "deployed"
        and (after[0] - before[0], after[1] - before[1]) == (3, 0) and b"SENTINEL" not in result.stdout + result.stderr,
        status=result.returncode, lookups=(after[0] - before[0], after[1] - before[1]), failure=failure(result))
    configure()


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
        deployment(lab)
    finally:
        server.terminate()
        server.communicate(timeout=5)
    passed = all(item["passed"] for item in lab.results)
    print(json.dumps({"passed": passed, "results": lab.results}, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
