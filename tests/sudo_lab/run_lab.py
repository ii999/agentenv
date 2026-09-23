#!/usr/bin/env python3
"""Run real sudo and OpenSSH compatibility probes in a disposable container."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pwd
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable


ROOT = Path("/opt/sudo-lab")
RUNTIME = Path("/run/sudo-lab")
ASKPASS = ROOT / "askpass.py"
SSHD_PORT = 2222
BASE_PASSWORD = "SudoLab-Base-Only-42!"
PREFIX_PASSWORD = "SudoLab-Prefix"
PASSWORD_255 = "A" * 254 + "5"
PASSWORD_256 = "B" * 255 + "6"


class Lab:
    def __init__(self) -> None:
        self.results: list[dict[str, Any]] = []
        self.details: dict[str, Any] = {}

    def record(self, name: str, passed: bool, **evidence: Any) -> None:
        self.results.append({"name": name, "passed": passed, "evidence": evidence})

    def command(
        self,
        argv: list[str],
        *,
        env: dict[str, str] | None = None,
        stdin: bytes | None = None,
        timeout: float = 15,
        user: str | None = None,
        pass_fds: tuple[int, ...] = (),
    ) -> subprocess.CompletedProcess[bytes]:
        command_env = os.environ.copy()
        if env:
            command_env.update(env)
        preexec_fn: Callable[[], None] | None = None
        if user:
            account = pwd.getpwnam(user)

            def demote() -> None:
                os.initgroups(account.pw_name, account.pw_gid)
                os.setgid(account.pw_gid)
                os.setuid(account.pw_uid)

            preexec_fn = demote
        return subprocess.run(
            argv,
            input=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=command_env,
            timeout=timeout,
            check=False,
            preexec_fn=preexec_fn,
            pass_fds=pass_fds,
        )

    def sudo(
        self,
        user: str,
        profile: str,
        command: list[str],
        *,
        marker: str,
        stdin: bytes | None = None,
        record: Path | None = None,
        askpass_mode: str = "newline",
        use_k: bool = True,
        pass_fds: tuple[int, ...] = (),
    ) -> subprocess.CompletedProcess[bytes]:
        argv = ["/usr/bin/sudo", "-A"]
        if use_k:
            argv.append("-k")
        argv.extend(
            ["-u", "root", "-p", f"[agentenv:S0:%p:{marker}]", "--", *command]
        )
        env = {
            "SUDO_ASKPASS": str(ASKPASS),
            "LAB_SECRET_PROFILE": profile,
            "LAB_ASKPASS_MODE": askpass_mode,
        }
        if record:
            env["LAB_ASKPASS_RECORD"] = str(record)
        return self.command(
            argv,
            env=env,
            stdin=stdin,
            user=user,
            pass_fds=pass_fds,
        )


def checked(argv: list[str], *, stdin: bytes | None = None) -> bytes:
    completed = subprocess.run(
        argv,
        input=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"fixture setup failed: {argv[0]} ({completed.returncode})")
    return completed.stdout


def install_accounts() -> None:
    accounts = {
        "labuser": BASE_PASSWORD,
        "prefixuser": PREFIX_PASSWORD,
        "length255": PASSWORD_255,
        "length256": PASSWORD_256,
    }
    for user in accounts:
        checked(["useradd", "--create-home", "--shell", "/bin/sh", user])
    payload = "".join(f"{user}:{password}\n" for user, password in accounts.items()).encode()
    checked(["chpasswd"], stdin=payload)

    sudoers = [
        "Defaults timestamp_type=global",
        "Defaults passwd_tries=1",
    ]
    for user in accounts:
        sudoers.extend(
            [
                f"{user} ALL=(root) NOPASSWD: /usr/bin/id -u",
                f"{user} ALL=(root) PASSWD: /opt/sudo-lab/bin/allowed fixed",
                f"{user} ALL=(root) PASSWD: /opt/sudo-lab/bin/binary_probe",
                f"{user} ALL=(root) PASSWD: /opt/sudo-lab/bin/fd_probe",
                f"{user} ALL=(root) PASSWD: /opt/sudo-lab/bin/long_running *",
            ]
        )
    policy = Path("/etc/sudoers.d/sudo-lab")
    policy.write_text("\n".join(sudoers) + "\n", encoding="utf-8")
    policy.chmod(0o440)
    checked(["visudo", "-cf", str(policy)])


def start_sshd() -> subprocess.Popen[bytes]:
    Path("/run/sshd").mkdir(mode=0o755, exist_ok=True)
    checked(["ssh-keygen", "-A"])
    client_key = RUNTIME / "client_key"
    checked(
        [
            "ssh-keygen",
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            str(client_key),
        ]
    )
    lab_account = pwd.getpwnam("labuser")
    ssh_dir = Path(lab_account.pw_dir) / ".ssh"
    ssh_dir.mkdir(mode=0o700)
    authorized_keys = ssh_dir / "authorized_keys"
    authorized_keys.write_bytes(client_key.with_suffix(".pub").read_bytes())
    authorized_keys.chmod(0o600)
    os.chown(ssh_dir, lab_account.pw_uid, lab_account.pw_gid)
    os.chown(authorized_keys, lab_account.pw_uid, lab_account.pw_gid)
    host_public = Path("/etc/ssh/ssh_host_ed25519_key.pub").read_text(encoding="ascii").split()
    known_hosts = RUNTIME / "known_hosts"
    known_hosts.write_text(
        f"[127.0.0.1]:{SSHD_PORT} {host_public[0]} {host_public[1]}\n",
        encoding="ascii",
    )
    config = RUNTIME / "sshd_config"
    config.write_text(
        "\n".join(
            [
                f"Port {SSHD_PORT}",
                "ListenAddress 127.0.0.1",
                "HostKey /etc/ssh/ssh_host_ed25519_key",
                "PasswordAuthentication yes",
                "KbdInteractiveAuthentication no",
                "PubkeyAuthentication yes",
                "UsePAM yes",
                "PermitRootLogin no",
                "PermitEmptyPasswords no",
                "PrintMotd no",
                "UseDNS no",
                "PidFile /run/sudo-lab/sshd.pid",
                "Subsystem sftp internal-sftp",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    process = subprocess.Popen(
        ["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", SSHD_PORT)) == 0:
                return process
        time.sleep(0.05)
    process.terminate()
    raise RuntimeError("fixture sshd did not start")


def ssh_password(
    lab: Lab,
    user: str,
    profile: str,
    *,
    mode: str = "newline",
    input_data: bytes | None = None,
    remote_command: list[str] | None = None,
    record: Path | None = None,
) -> subprocess.CompletedProcess[bytes]:
    env = {
        "SSH_ASKPASS": str(ASKPASS),
        "SSH_ASKPASS_REQUIRE": "force",
        "DISPLAY": "sudo-lab",
        "LAB_SECRET_PROFILE": profile,
        "LAB_ASKPASS_MODE": mode,
    }
    if record:
        env["LAB_ASKPASS_RECORD"] = str(record)
    argv = [
        "/usr/bin/ssh",
        "-F",
        "none",
        "-p",
        str(SSHD_PORT),
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        f"UserKnownHostsFile={RUNTIME / 'known_hosts'}",
        "-o",
        "GlobalKnownHostsFile=none",
        "-o",
        "PreferredAuthentications=password",
        "-o",
        "PasswordAuthentication=yes",
        "-o",
        "PubkeyAuthentication=no",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "IdentityAgent=none",
        "-o",
        "IdentityFile=none",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "RequestTTY=no",
        f"{user}@127.0.0.1",
        *(remote_command or ["/usr/bin/id", "-u"]),
    ]
    return lab.command(argv, env=env, stdin=input_data, timeout=10)


def parse_ssh_g(output: bytes) -> dict[str, list[str]]:
    options: dict[str, list[str]] = {}
    for raw_line in output.decode("utf-8").splitlines():
        key, _, value = raw_line.partition(" ")
        options.setdefault(key, []).append(value)
    return options


def test_sudo(lab: Lab) -> None:
    prompt_record = RUNTIME / "sudo-prompt.json"
    allowed = lab.sudo(
        "labuser",
        "base",
        [str(ROOT / "bin/allowed"), "fixed"],
        marker="prompt",
        record=prompt_record,
    )
    events = json.loads(prompt_record.read_text(encoding="utf-8"))
    payload = json.loads(allowed.stdout)
    lab.record(
        "sudo_real_askpass_and_account_marker",
        allowed.returncode == 0
        and payload == {"argv": ["fixed"], "euid": 0}
        and events[0]["prompt"] == "[agentenv:S0:labuser:prompt]",
        askpass_calls=len(events),
        expanded_prompt=events[0]["prompt"],
        target_euid=payload["euid"],
    )

    nopass_record = RUNTIME / "nopass.json"
    nopass = lab.sudo(
        "labuser",
        "unavailable",
        ["/usr/bin/id", "-u"],
        marker="nopass",
        record=nopass_record,
    )
    lab.record(
        "sudo_nopasswd_skips_resolution",
        nopass.returncode == 0 and nopass.stdout.strip() == b"0" and not nopass_record.exists(),
        askpass_called=nopass_record.exists(),
        target_euid=int(nopass.stdout.strip() or -1),
    )

    cache_record = RUNTIME / "cache.json"
    cache_runs = [
        lab.sudo(
            "labuser",
            "base",
            [str(ROOT / "bin/allowed"), "fixed"],
            marker=f"cache-{index}",
            record=cache_record,
        )
        for index in range(2)
    ]
    cache_events = json.loads(cache_record.read_text(encoding="utf-8"))
    lab.record(
        "sudo_k_forces_each_authentication",
        all(item.returncode == 0 for item in cache_runs) and len(cache_events) == 2,
        invocations=2,
        askpass_calls=len(cache_events),
    )

    denied_args = lab.sudo(
        "labuser", "base", [str(ROOT / "bin/allowed"), "other"], marker="deny-args"
    )
    denied_shell = lab.sudo(
        "labuser", "base", ["/bin/sh", "-c", str(ROOT / "bin/allowed")], marker="deny-shell"
    )
    lab.record(
        "sudoers_authorizes_direct_executable_and_exact_args",
        allowed.returncode == 0 and denied_args.returncode != 0 and denied_shell.returncode != 0,
        allowed_status=allowed.returncode,
        changed_args_status=denied_args.returncode,
        shell_status=denied_shell.returncode,
    )

    binary_input = bytes(range(256)) + b"\x00\xffleading\n"
    binary = lab.sudo(
        "labuser",
        "base",
        [str(ROOT / "bin/binary_probe")],
        marker="binary",
        stdin=binary_input,
    )
    expected_stdout = b"stdout:" + hashlib.sha256(binary_input).hexdigest().encode() + b"\x00"
    expected_stderr_suffix = b"stderr:" + str(len(binary_input)).encode() + b"\xff"
    lab.record(
        "sudo_askpass_keeps_binary_stdin_and_streams_separate",
        binary.returncode == 0
        and binary.stdout == expected_stdout
        and binary.stderr.endswith(expected_stderr_suffix),
        input_bytes=len(binary_input),
        input_sha256=hashlib.sha256(binary_input).hexdigest(),
        stdout_sha256=hashlib.sha256(binary.stdout).hexdigest(),
        stderr_has_expected_target_suffix=binary.stderr.endswith(expected_stderr_suffix),
    )

    fd_source = os.open("/dev/null", os.O_RDONLY)
    os.dup2(fd_source, 9)
    os.close(fd_source)
    try:
        fd_record = RUNTIME / "fd-askpass.json"
        fd_result = lab.sudo(
            "labuser",
            "base",
            [str(ROOT / "bin/fd_probe")],
            marker="fd",
            record=fd_record,
            pass_fds=(9,),
        )
    finally:
        os.close(9)
    fd_events = json.loads(fd_record.read_text(encoding="utf-8"))
    lab.record(
        "sudo_closes_extra_descriptors",
        fd_result.returncode == 0
        and json.loads(fd_result.stdout)["fd_9_open"] is False
        and fd_events[0]["fd_9_open"] is False,
        inherited_fd=9,
        askpass_fd_open=fd_events[0]["fd_9_open"],
        target_fd_open=json.loads(fd_result.stdout)["fd_9_open"],
    )

    for user, profile, byte_length in [
        ("length255", "length-255", 255),
        ("length256", "length-256", 256),
    ]:
        result = lab.sudo(
            user,
            profile,
            [str(ROOT / "bin/allowed"), "fixed"],
            marker=f"length-{byte_length}",
        )
        lab.record(
            f"sudo_askpass_password_{byte_length}_bytes",
            result.returncode == 0,
            credential_bytes=byte_length,
            reply_terminator_bytes=1,
            status=result.returncode,
        )

    truncated = lab.sudo(
        "prefixuser",
        "prefix-with-suffix",
        [str(ROOT / "bin/allowed"), "fixed"],
        marker="embedded-newline",
    )
    lab.record(
        "sudo_askpass_stops_at_embedded_newline",
        truncated.returncode == 0,
        candidate_shape="correct-prefix + LF + ignored-suffix",
        status=truncated.returncode,
    )
    carriage_truncated = lab.sudo(
        "prefixuser",
        "prefix-with-carriage-return",
        [str(ROOT / "bin/allowed"), "fixed"],
        marker="embedded-carriage-return",
    )
    lab.record(
        "sudo_askpass_stops_at_embedded_carriage_return",
        carriage_truncated.returncode == 0,
        candidate_shape="correct-prefix + CR + ignored-suffix",
        status=carriage_truncated.returncode,
    )
    unterminated = lab.sudo(
        "labuser",
        "base",
        [str(ROOT / "bin/allowed"), "fixed"],
        marker="unterminated",
        askpass_mode="unterminated",
    )
    lab.record(
        "sudo_askpass_accepts_eof_without_line_terminator",
        unterminated.returncode == 0,
        status=unterminated.returncode,
    )


def test_ssh_password(lab: Lab) -> None:
    record = RUNTIME / "ssh-prompt.json"
    base = ssh_password(lab, "labuser", "base", record=record)
    events = json.loads(record.read_text(encoding="utf-8"))
    lab.record(
        "ssh_forced_askpass_real_password_login",
        base.returncode == 0 and base.stdout.strip().isdigit() and len(events) == 1,
        askpass_calls=len(events),
        askpass_prompt_type=events[0]["prompt_type"],
        status=base.returncode,
    )
    for user, profile, byte_length in [
        ("length255", "length-255", 255),
        ("length256", "length-256", 256),
    ]:
        result = ssh_password(lab, user, profile)
        lab.record(
            f"ssh_askpass_password_{byte_length}_bytes",
            result.returncode == 0,
            credential_bytes=byte_length,
            reply_terminator_bytes=1,
            status=result.returncode,
        )
    unterminated = ssh_password(lab, "labuser", "base", mode="unterminated")
    lab.record(
        "ssh_askpass_accepts_eof_without_line_terminator",
        unterminated.returncode == 0,
        status=unterminated.returncode,
    )
    truncated = ssh_password(lab, "prefixuser", "prefix-with-suffix")
    lab.record(
        "ssh_askpass_stops_at_embedded_newline",
        truncated.returncode == 0,
        candidate_shape="correct-prefix + LF + ignored-suffix",
        status=truncated.returncode,
    )
    carriage_truncated = ssh_password(
        lab, "prefixuser", "prefix-with-carriage-return"
    )
    lab.record(
        "ssh_askpass_stops_at_embedded_carriage_return",
        carriage_truncated.returncode == 0,
        candidate_shape="correct-prefix + CR + ignored-suffix",
        status=carriage_truncated.returncode,
    )

    confirm_record = RUNTIME / "ssh-confirm.json"
    empty_known_hosts = RUNTIME / "empty_known_hosts"
    empty_known_hosts.write_bytes(b"")
    confirm = lab.command(
        [
            "/usr/bin/ssh",
            "-F",
            "none",
            "-p",
            str(SSHD_PORT),
            "-o",
            "StrictHostKeyChecking=ask",
            "-o",
            f"UserKnownHostsFile={empty_known_hosts}",
            "-o",
            "GlobalKnownHostsFile=none",
            "-o",
            "PreferredAuthentications=password",
            "-o",
            "NumberOfPasswordPrompts=1",
            "labuser@127.0.0.1",
            "/usr/bin/true",
        ],
        env={
            "SSH_ASKPASS": str(ASKPASS),
            "SSH_ASKPASS_REQUIRE": "force",
            "DISPLAY": "sudo-lab",
            "LAB_SECRET_PROFILE": "unavailable",
            "LAB_ASKPASS_RECORD": str(confirm_record),
        },
        timeout=5,
    )
    confirm_events = json.loads(confirm_record.read_text(encoding="utf-8"))
    confirm_shape = "authenticity of host" in confirm_events[0]["prompt"].lower()
    lab.record(
        "ssh_unknown_host_key_confirmation_can_lack_prompt_type",
        confirm.returncode == 255
        and len(confirm_events) == 1
        and confirm_events[0]["prompt_type"] is None
        and confirm_shape
        and confirm_events[0]["profile"] == "unavailable",
        askpass_calls=len(confirm_events),
        prompt_type=confirm_events[0]["prompt_type"],
        prompt_shape="host-key-confirmation" if confirm_shape else "unexpected",
        password_profile_available=False,
        status=confirm.returncode,
    )

    binary_input = bytes(range(256)) + b"\x00\xffssh-stdin"
    stdin_result = ssh_password(
        lab,
        "labuser",
        "base",
        input_data=binary_input,
        remote_command=["/usr/bin/sha256sum"],
    )
    lab.record(
        "ssh_forced_askpass_keeps_binary_stdin_separate",
        stdin_result.returncode == 0
        and stdin_result.stdout.split()[0].decode() == hashlib.sha256(binary_input).hexdigest(),
        input_bytes=len(binary_input),
        input_sha256=hashlib.sha256(binary_input).hexdigest(),
        remote_sha256=stdin_result.stdout.split()[0].decode() if stdin_result.stdout else None,
    )


def write_ssh_configs() -> tuple[Path, Path, Path]:
    include = RUNTIME / "ssh_include"
    include.write_text(
        "Host lab-alias\n"
        "    User labuser\n"
        f"    Port {SSHD_PORT}\n",
        encoding="utf-8",
    )
    config = RUNTIME / "ssh_config"
    config.write_text(
        "Host lab-alias\n"
        "    HostName 127.0.0.1\n"
        f"    Include {include}\n"
        "Match host 127.0.0.1 user labuser\n"
        "    ServerAliveInterval 17\n",
        encoding="utf-8",
    )
    home = RUNTIME / "hostile-home"
    (home / ".ssh").mkdir(parents=True)
    (home / ".ssh/config").write_text(
        "Host *\n    HostName config-must-not-apply.invalid\n    User wrong-user\n    Port 2299\n",
        encoding="utf-8",
    )
    jump = RUNTIME / "jump_config"
    common = (
        "    BatchMode yes\n"
        "    PreferredAuthentications publickey\n"
        "    PasswordAuthentication no\n"
        "    KbdInteractiveAuthentication no\n"
        "    StrictHostKeyChecking yes\n"
        f"    UserKnownHostsFile {RUNTIME / 'known_hosts'}\n"
        "    GlobalKnownHostsFile none\n"
        "    UpdateHostKeys no\n"
        "    ControlMaster no\n"
        "    ControlPath none\n"
        "    ForwardAgent no\n"
        "    ForwardX11 no\n"
        "    PermitLocalCommand no\n"
        "    IdentitiesOnly yes\n"
        "    IdentityAgent none\n"
        f"    IdentityFile {RUNTIME / 'client_key'}\n"
    )
    jump.write_text(
        "Host final\n"
        "    HostName 127.0.0.1\n"
        "    User labuser\n"
        f"    Port {SSHD_PORT}\n"
        "    ProxyJump jump\n"
        + common
        + "Host jump\n"
        "    HostName 127.0.0.1\n"
        "    User labuser\n"
        f"    Port {SSHD_PORT}\n"
        + common,
        encoding="utf-8",
    )
    return config, home, jump


def test_ssh_config(lab: Lab) -> None:
    config, hostile_home, jump_config = write_ssh_configs()
    evaluated = lab.command(["ssh", "-G", "-F", str(config), "lab-alias"])
    options = parse_ssh_g(evaluated.stdout)
    lab.record(
        "ssh_g_resolves_alias_include_and_match",
        evaluated.returncode == 0
        and options.get("hostname") == ["127.0.0.1"]
        and options.get("user") == ["labuser"]
        and options.get("port") == [str(SSHD_PORT)]
        and options.get("serveraliveinterval") == ["17"],
        hostname=options.get("hostname"),
        user=options.get("user"),
        port=options.get("port"),
        match_value=options.get("serveraliveinterval"),
    )

    explicit = lab.command(
        [
            "ssh",
            "-G",
            "-F",
            "none",
            "-o",
            "HostName=127.0.0.1",
            "-o",
            "User=labuser",
            "-o",
            f"Port={SSHD_PORT}",
            "-o",
            "IdentityFile=none",
            "-o",
            "IdentityFile=/run/sudo-lab/declared-key",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "explicit-name",
        ],
        env={"HOME": str(hostile_home)},
    )
    explicit_options = parse_ssh_g(explicit.stdout)
    identity_files = explicit_options.get("identityfile", [])
    lab.record(
        "ssh_f_none_ignores_user_config_and_keeps_explicit_keys",
        explicit.returncode == 0
        and explicit_options.get("hostname") == ["127.0.0.1"]
        and explicit_options.get("user") == ["labuser"]
        and explicit_options.get("port") == [str(SSHD_PORT)]
        and identity_files == ["none", "/run/sudo-lab/declared-key"]
        and explicit_options.get("identitiesonly") == ["yes"]
        and explicit_options.get("identityagent") == ["none"],
        hostname=explicit_options.get("hostname"),
        user=explicit_options.get("user"),
        port=explicit_options.get("port"),
        identity_files=identity_files,
        identities_only=explicit_options.get("identitiesonly"),
        identity_agent=explicit_options.get("identityagent"),
    )

    hop = lab.command(["ssh", "-G", "-F", str(jump_config), "jump"])
    hop_options = parse_ssh_g(hop.stdout)
    required = {
        "batchmode": "yes",
        "preferredauthentications": "publickey",
        "passwordauthentication": "no",
        "kbdinteractiveauthentication": "no",
        "stricthostkeychecking": "true",
        "updatehostkeys": "false",
        "controlmaster": "false",
        "forwardagent": "no",
        "forwardx11": "no",
        "permitlocalcommand": "no",
        "identitiesonly": "yes",
        "identityagent": "none",
    }
    observed = {key: hop_options.get(key, [None])[0] for key in required}
    configured_forwarding = {
        key: hop_options.get(key)
        for key in ("localforward", "remoteforward", "dynamicforward")
    }
    lab.record(
        "proxyjump_hop_effective_options_meet_frozen_predicates",
        hop.returncode == 0
        and all(observed[key] == expected for key, expected in required.items())
        and hop_options.get("userknownhostsfile") == [str(RUNTIME / "known_hosts")]
        and hop_options.get("globalknownhostsfile") == ["none"]
        and hop_options.get("identityfile") == [str(RUNTIME / "client_key")]
        and all(value is None for value in configured_forwarding.values()),
        predicates=observed,
        user_known_hosts=hop_options.get("userknownhostsfile"),
        global_known_hosts=hop_options.get("globalknownhostsfile"),
        control_path_when_master_disabled=hop_options.get("controlpath"),
        identity_files=hop_options.get("identityfile"),
        configured_forwarding=configured_forwarding,
    )

    unsafe_config = RUNTIME / "unsafe_jump_config"
    unsafe_text = jump_config.read_text(encoding="utf-8").replace(
        "Host jump\n    HostName", "Host jump\n    BatchMode no\n    HostName"
    )
    unsafe_config.write_text(unsafe_text, encoding="utf-8")
    unsafe_hop = parse_ssh_g(
        lab.command(["ssh", "-G", "-F", str(unsafe_config), "jump"]).stdout
    )
    failed_predicates = [
        key for key, expected in required.items() if unsafe_hop.get(key, [None])[0] != expected
    ]
    lab.record(
        "proxyjump_unsafe_inner_hop_is_detectable_pre_connection",
        "batchmode" in failed_predicates,
        failed_predicates=failed_predicates,
    )

    final_options = parse_ssh_g(
        lab.command(["ssh", "-G", "-F", str(jump_config), "final"]).stdout
    )
    lab.record(
        "password_mode_can_reject_proxyjump_from_effective_config_before_lookup",
        final_options.get("proxyjump") == ["jump"],
        effective_proxyjump=final_options.get("proxyjump"),
        policy="password mode rejects any effective ProxyJump or ProxyCommand",
    )

    conforming = lab.command(
        ["ssh", "-F", str(jump_config), "final", "/usr/bin/true"], timeout=10
    )
    lab.record(
        "proxyjump_conforming_key_route_succeeds",
        conforming.returncode == 0,
        status=conforming.returncode,
        hops=1,
        authentication="explicit fixture identity file, no agent",
    )

    debug = lab.command(
        [
            "ssh",
            "-vv",
            "-F",
            str(jump_config),
            "-o",
            "ConnectTimeout=1",
            "-o",
            "BatchMode=yes",
            "-o",
            "PasswordAuthentication=no",
            "final",
            "/usr/bin/true",
        ],
        timeout=5,
    )
    debug_text = debug.stderr.decode("utf-8", errors="replace")
    match = re.search(r"Executing proxy command: exec (.+)", debug_text)
    child = match.group(1) if match else ""
    lab.record(
        "proxyjump_child_invocation_does_not_inherit_outer_o_options",
        bool(match)
        and " -W " in child
        and " jump" in child
        and "BatchMode=yes" not in child
        and "PasswordAuthentication=no" not in child,
        child_argv_debug=child,
        outer_status=debug.returncode,
    )


def wait_for_pid_file(path: Path, timeout: float = 3) -> int:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.read_text(encoding="ascii"):
            return int(path.read_text(encoding="ascii"))
        time.sleep(0.05)
    raise RuntimeError(f"pid file not created: {path}")


def process_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def test_cancellation(lab: Lab) -> None:
    account = pwd.getpwnam("labuser")

    def demote_and_session() -> None:
        os.setsid()
        os.initgroups(account.pw_name, account.pw_gid)
        os.setgid(account.pw_gid)
        os.setuid(account.pw_uid)

    env = os.environ.copy()
    env.update(
        {
            "SUDO_ASKPASS": str(ASKPASS),
            "LAB_SECRET_PROFILE": "base",
            "LAB_ASKPASS_MODE": "newline",
        }
    )
    foreground_pid_file = RUNTIME / "foreground.pid"
    foreground = subprocess.Popen(
        [
            "/usr/bin/sudo",
            "-A",
            "-k",
            "-u",
            "root",
            "-p",
            "[agentenv:S0:%p:cancel-foreground]",
            "--",
            str(ROOT / "bin/long_running"),
            str(foreground_pid_file),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        preexec_fn=demote_and_session,
    )
    target_pid = wait_for_pid_file(foreground_pid_file)
    foreground.send_signal(signal.SIGTERM)
    try:
        foreground.wait(timeout=3)
        sudo_reaped = True
    except subprocess.TimeoutExpired:
        sudo_reaped = False
        foreground.kill()
        foreground.wait(timeout=2)
    time.sleep(0.1)
    target_alive = process_alive(target_pid)
    if target_alive:
        os.kill(target_pid, signal.SIGKILL)
    lab.record(
        "sudo_cancel_foreground_forwards_and_reaps",
        sudo_reaped and not target_alive,
        sudo_reaped=sudo_reaped,
        target_alive_after_cancel=target_alive,
        sudo_status=foreground.returncode,
    )

    detached_pid_file = RUNTIME / "detached.pid"
    detached = subprocess.Popen(
        [
            "/usr/bin/sudo",
            "-A",
            "-k",
            "-u",
            "root",
            "-p",
            "[agentenv:S0:%p:cancel-detached]",
            "--",
            str(ROOT / "bin/long_running"),
            str(detached_pid_file),
            "detached",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        preexec_fn=demote_and_session,
    )
    detached_target_pid = wait_for_pid_file(detached_pid_file)
    detached.send_signal(signal.SIGTERM)
    try:
        detached.wait(timeout=3)
        detached_sudo_reaped = True
    except subprocess.TimeoutExpired:
        detached_sudo_reaped = False
        detached.kill()
        detached.wait(timeout=2)
    time.sleep(0.1)
    detached_alive = process_alive(detached_target_pid)
    if detached_alive:
        os.kill(detached_target_pid, signal.SIGKILL)
    lab.record(
        "sudo_cancel_cannot_guarantee_detached_descendant_termination",
        detached_alive,
        sudo_reaped=detached_sudo_reaped,
        detached_descendant_alive_after_cancel=detached_alive,
        cleanup="lab root sent SIGKILL after measurement",
    )


def collect_versions(lab: Lab) -> None:
    packages = ["sudo", "openssh-client", "openssh-server", "libpam-modules"]
    versions: dict[str, str] = {}
    for package in packages:
        result = lab.command(["dpkg-query", "-W", "-f=${Version}", package])
        versions[package] = result.stdout.decode("utf-8") if result.returncode == 0 else "absent"
    lab.details["versions"] = {
        "base_image": "postgres:17",
        "base_image_digest": "sha256:67f41722b7a8cbdb868a44a4995c846eddfdc2973bccb291ce937dce88ad5675",
        "os_release": Path("/etc/os-release").read_text(encoding="utf-8").splitlines(),
        "kernel": lab.command(["uname", "-srmo"]).stdout.decode().strip(),
        "packages": versions,
        "sudo": lab.command(["sudo", "-V"]).stdout.decode().splitlines()[0],
        "ssh": lab.command(["ssh", "-V"]).stderr.decode().strip(),
        "python": sys.version.splitlines()[0],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    RUNTIME.mkdir(mode=0o1777, exist_ok=True)
    RUNTIME.chmod(0o1777)
    install_accounts()
    sshd = start_sshd()
    lab = Lab()
    try:
        collect_versions(lab)
        test_sudo(lab)
        test_ssh_password(lab)
        test_ssh_config(lab)
        test_cancellation(lab)
    finally:
        sshd.terminate()
        try:
            sshd.wait(timeout=2)
        except subprocess.TimeoutExpired:
            sshd.kill()
            sshd.wait(timeout=2)

    lab.details["scope"] = {
        "platform": "disposable Linux container only",
        "host_changes": False,
        "published_ports": False,
        "remote_hosts": False,
        "credential_origin": "synthetic fixture constants generated in-container",
    }
    report = {
        "schema_version": 1,
        "passed": all(result["passed"] for result in lab.results),
        "summary": {
            "passed": sum(result["passed"] for result in lab.results),
            "failed": sum(not result["passed"] for result in lab.results),
            "total": len(lab.results),
        },
        **lab.details,
        "results": lab.results,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report["summary"], sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
