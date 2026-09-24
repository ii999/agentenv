#!/usr/bin/env python3
"""Native Windows OpenSSH against a disposable loopback protocol fixture.

This verifies the Windows client and askpass, not real Unix sudo. The separate
Linux container labs cover the actual remote helper/sudo. No OS accounts,
services, firewall rules or existing keys are created or modified here.
"""
from __future__ import annotations

import json
import logging
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import time
import tomllib

import paramiko

logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / "target/debug/agentenv.exe"
PROBE = ROOT / "target/debug/test-probe.exe"
BUILD = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
SYNTHETIC = "  Windows-fixture-密码🔑  "
ALIAS = "agentenv-native-fixture"


def exact(channel, count):
    data = bytearray()
    while len(data) < count:
        part = channel.recv(count - len(data))
        if not part:
            raise EOFError("fixture channel closed")
        data.extend(part)
    return bytes(data)


def receive(channel):
    magic, version, kind, request, size = struct.unpack(">4sHHQI", exact(channel, 20))
    if magic != b"AGEP" or version != 1 or request == 0 or size > 65536:
        raise ValueError("invalid fixture frame")
    return kind, request, exact(channel, size)


def send(channel, kind, request, data):
    payload = json.dumps(data, separators=(",", ":")).encode() if isinstance(data, dict) else data
    channel.sendall(struct.pack(">4sHHQI", b"AGEP", 1, kind, request, len(payload)) + payload)


class Peer(paramiko.ServerInterface):
    def __init__(self, key):
        self.key = key
        self.passwords = 0
        self.public_keys = 0
        self.execute = threading.Event()

    def get_allowed_auths(self, username):
        return "publickey,password"

    def check_auth_password(self, username, password):
        self.passwords += 1
        return paramiko.AUTH_SUCCESSFUL if username == "fixture" and password == SYNTHETIC else paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        if username == "fixture" and key == self.key:
            self.public_keys += 1
            return paramiko.AUTH_SUCCESSFUL
        return paramiko.AUTH_FAILED

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_exec_request(self, channel, command):
        if command != b"/fixture/agentenv-sudo-helper --serve":
            return False
        self.execute.set()
        return True


def serve(listener, host_key, peer, errors):
    transport = None
    try:
        client, _ = listener.accept()
        transport = paramiko.Transport(client)
        transport.add_server_key(host_key)
        transport.start_server(server=peer)
        channel = transport.accept(15)
        if channel is None or not peer.execute.wait(10):
            return
        channel.settimeout(15)
        kind, request, payload = receive(channel)
        if kind != 1 or json.loads(payload)["mode"] != "check":
            raise ValueError("unexpected fixture operation")
        send(channel, 2, request, {"build": BUILD, "protocol": 1, "platform": "linux",
             "auth_user": "fixture", "uid": 1000, "cwd": "/fixture", "password_limit": 255,
             "features": ["one-shot-auth", "binary-streams", "credits", "cancel"]})
        channel.send_exit_status(0)
        channel.shutdown_write()
        # SSH EOF is only a half-close. A real completed helper sends CLOSE
        # too; Win32 OpenSSH correctly waits for it before exiting. Keep the
        # transport alive so the queued Ready/status/CLOSE frames can drain.
        channel.close()
        # Keep the SSH transport alive until the client consumes Ready and closes.
        until = time.monotonic() + 10
        while transport.is_active() and time.monotonic() < until:
            time.sleep(0.01)
    except (EOFError, paramiko.SSHException, OSError):
        # Untrusted host / rejected password intentionally closes during setup.
        pass
    except Exception as exc:
        errors.append(type(exc).__name__)
    finally:
        if transport is not None:
            transport.close()
        listener.close()


def run_case(directory, method, trusted=True, correct=True):
    host_key = paramiko.ECDSAKey.generate()
    client_key = paramiko.ECDSAKey.generate()
    key_path = directory / "client-key"
    client_key.write_private_key_file(str(key_path))
    known = directory / "known-hosts"
    known.write_text(f"{ALIAS} {host_key.get_name()} {host_key.get_base64()}\n" if trusted else "", encoding="utf-8")
    marker = directory / "lookup-marker"
    marker.unlink(missing_ok=True)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(20)
    port = listener.getsockname()[1]
    value = SYNTHETIC if correct else "synthetic-wrong-password"
    argv = [str(PROBE), "--resolver-fixture", "delayed", value, str(marker)]
    auth = 'credential = "credential://login"' if method == "password" else f"identity_files = {json.dumps([str(key_path)])}\nuse_agent = false"
    config = directory / "config.toml"
    config.write_text(f'''version = 1
default_profile = "lab"
[credentials.login]
description = "Disposable login fixture"
provider = "command"
argv = {json.dumps(argv, ensure_ascii=False)}
usages = ["ssh-password"]
[credentials.sudo]
description = "Never resolved by --check"
provider = "command"
argv = {json.dumps([str(PROBE), "--resolver-fixture", "failure"])}
usages = ["sudo"]
[profiles.lab]
description = "Native client lab"
[profiles.lab.admin]
description = "Loopback fixture"
kind = "sudo-target"
[profiles.lab.admin.sudo]
transport = "ssh"
credential = "credential://sudo"
auth_user = "fixture"
run_as = "root"
sudo_path = "/usr/bin/sudo"
[profiles.lab.admin.sudo.ssh]
mode = "explicit"
hostname = "127.0.0.1"
user = "fixture"
port = {port}
host_key_alias = "{ALIAS}"
known_hosts_file = {json.dumps(str(known))}
helper_path = "/fixture/agentenv-sudo-helper"
[profiles.lab.admin.sudo.ssh.auth]
method = "{method}"
{auth}
''', encoding="utf-8")
    peer = Peer(client_key)
    errors = []
    thread = threading.Thread(target=serve, args=(listener, host_key, peer, errors), daemon=True)
    thread.start()
    env = {**os.environ, "AGENTENV_FILE": str(config), "AGENTENV_NO_PROJECT": "1"}
    result = subprocess.run([str(APP), "sudo", "--with", "admin", "--check", "--json",
        "--connect-timeout-secs", "10", "--auth-timeout-secs", "10"],
        input=b"", capture_output=True, env=env, timeout=30)
    thread.join(21)
    output = result.stdout + result.stderr
    assert SYNTHETIC.encode() not in output and value.encode() not in output, "fixture value leaked"
    expected_success = trusted and correct
    assert (result.returncode == 0) == expected_success, f"native client status {result.returncode}: {result.stderr.decode(errors='replace')}"
    assert not errors and not thread.is_alive(), "fixture server did not close cleanly"
    assert marker.exists() == (trusted and method == "password"), "unexpected credential lookup"
    assert peer.passwords == (1 if trusted and method == "password" else 0), "password was retried or not received"
    if expected_success:
        assert json.loads(result.stdout)["status"] == "ready"
    else:
        assert not result.stdout, "failed JSON command emitted output"


def main():
    if os.name != "nt":
        raise SystemExit("This lab requires native Windows")
    native = Path(os.environ["SystemRoot"]) / "System32/OpenSSH"
    os.environ["PATH"] = str(native) + os.pathsep + os.environ["PATH"]
    version = subprocess.run([str(native / "ssh.exe"), "-V"], capture_output=True, check=True)
    print(version.stderr.decode().strip())
    with tempfile.TemporaryDirectory(prefix="agentenv native fixture ") as path:
        directory = Path(path)
        for method, trusted, correct in [("password", True, True), ("password", True, False),
                                          ("password", False, True), ("publickey", True, True)]:
            run_case(directory, method, trusted, correct)
            print(f"PASS native SSH: {method}, trusted={trusted}, correct={correct}")


if __name__ == "__main__":
    main()
