#!/usr/bin/env python3
"""Exercise a built server image using disposable Docker resources.

Requires Linux, Docker, Python 3.10+, and openssl. No Python packages are needed.
The test covers native HTTPS and an optional temporary Nginx TLS proxy using
private bridges, named data volumes, and disposable certificates. Only random
loopback host ports are published. It never publishes images or contacts a
production server. Bootstrap codes, passwords, cookies, and raw
container output stay in memory and are deliberately excluded from diagnostics.
"""

import argparse
import base64
import errno
import hashlib
import http.client
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
import re
import secrets
import select
import shutil
import signal
import socket
import ssl
import sqlite3
import subprocess
import sys
import tempfile
import time


DEFAULT_COMMAND = ["run"]
DEFAULT_ENV = {
    "ERGENT_DATABASE": "/data/server.db",
    "ERGENT_LISTEN": "0.0.0.0:7777",
    "ERGENT_WEB": "/app/web",
    "ERGENT_TLS_MODE": "direct",
}
SUBPROTOCOL = "ergent.preview.v3"
SETUP_CODE = re.compile(rb"Setup code: ([A-Za-z0-9_-]{43})(?:\s|$)")


class SmokeFailure(Exception):
    """An intentionally secret-free diagnostic safe to display in CI."""


def require(condition, message):
    if not condition:
        raise SmokeFailure(message)


def command(args, *, check=True, timeout=60, input=None):
    # Do not use check=True or print CalledProcessError: subprocess exceptions
    # can otherwise disclose captured terminal output or arguments.
    try:
        result = subprocess.run(
            args, stdin=subprocess.DEVNULL if input is None else None, input=input,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, check=False,
        )
    except subprocess.TimeoutExpired:
        raise SmokeFailure("A test command timed out; captured output withheld.") from None
    if check:
        require(result.returncode == 0,
                f"A test command failed (exit {result.returncode}); captured output withheld.")
    return result


def terminal_command(args, timeout=30):
    """Supply a real controlling PTY, capturing it without a terminal transcript."""
    # Imported here so offline unit tests remain importable on Windows.
    import fcntl
    import pty
    import termios

    master, slave = pty.openpty()
    child = None
    output = bytearray()

    def claim_terminal():
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    try:
        child = subprocess.Popen(
            args, stdin=slave, stdout=slave, stderr=slave,
            start_new_session=True, preexec_fn=claim_terminal,
        )
        os.close(slave)
        slave = None
        deadline = time.monotonic() + timeout
        while True:
            require(time.monotonic() < deadline,
                    "The operator-terminal command timed out; captured output withheld.")
            readable, _, _ = select.select([master], [], [], 0.1)
            if not readable:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            if not chunk:
                break
            output.extend(chunk)
            require(len(output) <= 1024 * 1024, "Unexpectedly large terminal output.")
        try:
            status = child.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            raise SmokeFailure("The operator-terminal command did not exit.") from None
        require(status == 0,
                f"The operator-terminal command failed (exit {status}); output withheld.")
        return bytes(output)
    finally:
        if child is not None and child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        os.close(master)
        if slave is not None:
            os.close(slave)


def extract_code(output):
    matches = SETUP_CODE.findall(output)
    require(len(matches) == 1, "The real operator terminal did not receive exactly one setup code.")
    return matches[0].decode("ascii")


def require_private_output(output, private_values=()):
    require(b"Setup code:" not in output, "A setup-code marker leaked into ordinary output.")
    for value in private_values:
        require(value.encode() not in output, "A test credential leaked into ordinary output.")


class Client:
    def __init__(self, port, *, tls=None, host="localhost"):
        self.port = port
        self.tls = tls
        self.host = host
        self.origin = f"{'https' if tls else 'http'}://{host}:{port}"
        self.cookie = ""
        self.csrf = ""

    def request(self, path, *, method="GET", data=None, headers=None):
        outgoing = dict(headers or {})
        if self.cookie:
            outgoing["Cookie"] = self.cookie
        body = None
        if data is not None:
            outgoing["Content-Type"] = "application/json"
            body = json.dumps(data).encode()
        if self.tls:
            connection = http.client.HTTPSConnection(
                self.host, self.port, timeout=5, context=self.tls,
            )
        else:
            connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            connection.request(method, path, body=body, headers=outgoing)
            response = connection.getresponse()
            response_headers = {key.lower(): value for key, value in response.getheaders()}
            content = response.read()
            cookie = response_headers.get("set-cookie")
            if cookie:
                parsed = SimpleCookie(cookie)
                require("ergent_session" in parsed, "Session response omitted its cookie.")
                session = parsed["ergent_session"]
                require(bool(session["httponly"]) and session["samesite"] == "Strict",
                        "The session cookie is missing browser security attributes.")
                require(not self.tls or bool(session["secure"]),
                        "An HTTPS session cookie is missing Secure.")
                self.cookie = f"ergent_session={session.value}"
            return response.status, response_headers, content
        finally:
            connection.close()

    def json(self, path, *, expected=200, **kwargs):
        status, _, body = self.request(path, **kwargs)
        require(status == expected, f"{path} returned HTTP {status}; expected {expected}.")
        try:
            return json.loads(body)
        except (ValueError, UnicodeDecodeError):
            raise SmokeFailure(f"{path} did not return valid JSON.") from None

    def get_csrf(self):
        data = self.json("/api/v1/auth/csrf")
        self.csrf = data["data"]["csrfToken"]
        require(bool(self.cookie) and bool(self.csrf), "CSRF bootstrap did not establish a session.")

    def write(self, path, data, *, expected=200, origin=None):
        return self.json(path, method="POST", data=data, expected=expected, headers={
            "Origin": origin or self.origin, "X-CSRF-Token": self.csrf,
        })

    def login(self, username, password):
        self.get_csrf()
        old_cookie = self.cookie
        result = self.write("/api/v1/auth/login", {"username": username, "password": password})
        require(result["data"]["user"]["username"] == username, "Login returned the wrong account.")
        require(self.cookie != old_cookie, "Login did not rotate the session cookie.")
        self.csrf = result["data"]["csrfToken"]
        me = self.json("/api/v1/auth/me")
        require(me["data"]["username"] == username, "The login session was not authenticated.")


def parse_upgrade(response):
    try:
        lines = response.decode("ascii").split("\r\n")
        status = int(lines[0].split()[1])
        headers = dict(line.split(":", 1) for line in lines[1:] if line)
        return status, {key.lower(): value.strip() for key, value in headers.items()}
    except (ValueError, IndexError, UnicodeDecodeError):
        raise SmokeFailure("The reverse proxy returned an invalid WebSocket handshake.") from None


def websocket(client, *, authenticated=True, expected=101, during_open=None):
    key = base64.b64encode(secrets.token_bytes(16)).decode()
    lines = [
        "GET /api/v1/ws/browser HTTP/1.1", f"Host: {client.host}:{client.port}",
        "Upgrade: websocket", "Connection: Upgrade", "Sec-WebSocket-Version: 13",
        f"Sec-WebSocket-Key: {key}", f"Sec-WebSocket-Protocol: {SUBPROTOCOL}",
        f"Origin: {client.origin}",
    ]
    if authenticated:
        require(bool(client.cookie), "WebSocket authentication requires a login cookie.")
        lines.append(f"Cookie: {client.cookie}")
    with socket.create_connection(("127.0.0.1", client.port), timeout=5) as raw:
        with client.tls.wrap_socket(raw, server_hostname=client.host) as connection:
            connection.sendall(("\r\n".join(lines) + "\r\n\r\n").encode())
            response = bytearray()
            while b"\r\n\r\n" not in response:
                data = connection.recv(4096)
                require(bool(data), "The WebSocket connection closed before its handshake.")
                response.extend(data)
                require(len(response) < 65536, "The WebSocket handshake was too large.")
            status, headers = parse_upgrade(bytes(response).split(b"\r\n\r\n", 1)[0])
            require(status == expected, f"WebSocket returned HTTP {status}; expected {expected}.")
            if expected == 101:
                accept = base64.b64encode(hashlib.sha1(
                    (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()
                ).digest()).decode()
                require(headers.get("sec-websocket-accept") == accept,
                        "The WebSocket accept key was invalid.")
                require(headers.get("sec-websocket-protocol") == SUBPROTOCOL,
                        "The WebSocket subprotocol was not negotiated.")
                require(headers.get("upgrade", "").lower() == "websocket",
                        "The reverse proxy did not forward the WebSocket upgrade.")
                if during_open:
                    during_open()
                    # Nginx must close the upgraded connection when the backend
                    # drains it. An open socket will time out and fail the test.
                    remaining = 0
                    while True:
                        data = connection.recv(65536)
                        if not data:
                            break
                        remaining += len(data)
                        require(remaining < 1024 * 1024, "The stopped WebSocket kept sending data.")
                else:
                    # RFC 6455 empty, masked close frame.
                    connection.sendall(b"\x88\x80" + secrets.token_bytes(4))


def proxy_configuration(backend):
    return f"""events {{}}
http {{
    access_log off;
    resolver 127.0.0.11 valid=1s;
    server {{
        listen 443 ssl;
        server_name localhost;
        ssl_certificate /etc/nginx/test-tls/cert.pem;
        ssl_certificate_key /etc/nginx/test-tls/key.pem;
        add_header X-Smoke-Proxy-Peer $remote_addr always;
        location / {{
            set $backend "{backend}:7777";
            proxy_pass http://$backend;
            proxy_http_version 1.1;
            proxy_set_header Host $http_host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection "upgrade";
        }}
    }}
}}
"""


def make_certificates(folder):
    command([
        "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
        "-keyout", str(folder / "ca.key"), "-out", str(folder / "ca.pem"),
        "-subj", "/CN=Ergent Docker smoke CA",
        "-addext", "basicConstraints=critical,CA:TRUE",
        "-addext", "keyUsage=critical,keyCertSign,cRLSign",
    ])
    command([
        "openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
        "-keyout", str(folder / "key.pem"), "-out", str(folder / "leaf.csr"),
        "-subj", "/CN=localhost",
    ])
    (folder / "leaf.ext").write_text(
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
        "extendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n", encoding="ascii",
    )
    command([
        "openssl", "x509", "-req", "-in", str(folder / "leaf.csr"),
        "-CA", str(folder / "ca.pem"), "-CAkey", str(folder / "ca.key"), "-CAcreateserial",
        "-days", "1", "-extfile", str(folder / "leaf.ext"), "-out", str(folder / "leaf.pem"),
    ])
    (folder / "cert.pem").write_bytes((folder / "leaf.pem").read_bytes() + (folder / "ca.pem").read_bytes())


class SmokeTest:
    def __init__(self, image, proxy_image, architecture, agent_binary=None):
        self.image = image
        self.proxy_image = proxy_image
        self.architecture = architecture
        self.agent_binary = agent_binary
        prefix = f"ergent-smoke-{secrets.token_hex(6)}"
        self.server = prefix + "-server"
        self.proxy = prefix + "-proxy"
        self.volume = prefix + "-data"
        self.network = prefix + "-network"
        self.private_values = []
        self.stage = "prerequisites"

    def docker(self, *args, **kwargs):
        return command(["docker", *args], **kwargs)

    def inspect(self, name):
        return json.loads(self.docker("inspect", name).stdout)[0]

    def logs_are_private(self):
        logs = self.docker("logs", self.server)
        require_private_output(logs.stdout + logs.stderr, self.private_values)

    def start(self, origin=None, trusted_proxy=None, *, tls=None, tls_dir=None, host_port="",
              host="localhost", mode=None):
        args = [
            "run", "--detach", "--name", self.server, "--network", self.network,
            "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
            "--mount", f"type=volume,source={self.volume},target=/data",
            "--publish", f"127.0.0.1:{host_port}:7777", "--health-interval", "1s",
            "--health-start-period", "2s",
            "--env", f"ERGENT_TLS_MODE={mode or ('direct' if tls_dir else 'proxy')}",
        ]
        args.extend(["--env", f"ERGENT_ORIGIN={origin or 'http://127.0.0.1:7777'}"])
        if trusted_proxy:
            args.extend(["--env", f"ERGENT_TRUSTED_PROXY={trusted_proxy}"])
        if tls_dir:
            args.extend([
                "--mount", f"type=bind,source={tls_dir},target=/test-tls,readonly",
                "--env", "ERGENT_TLS_CERT=/test-tls/cert.pem",
                "--env", "ERGENT_TLS_KEY=/test-tls/key.pem",
            ])
        self.docker(*args, self.image)
        port = self.inspect(self.server)["NetworkSettings"]["Ports"]["7777/tcp"][0]["HostPort"]
        client = Client(int(port), tls=tls, host=host)
        self.wait_ready(client)
        return client

    def wait_ready(self, client):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            state = self.inspect(self.server)["State"]
            require(state["Running"], "The server exited before becoming ready; logs withheld.")
            try:
                status, _, body = client.request("/health/ready")
                if status == 200 and body == b"ok" and state.get("Health", {}).get("Status") == "healthy":
                    return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.5)
        raise SmokeFailure("The server or its Docker HEALTHCHECK did not become ready.")

    def stop(self):
        self.logs_are_private()
        self.docker("stop", "--signal", "SIGTERM", "--timeout", "20", self.server, timeout=30)
        state = self.inspect(self.server)["State"]
        require(not state["Running"] and state["ExitCode"] == 0 and not state["OOMKilled"],
                "SIGTERM did not produce a clean exit (Docker may have needed SIGKILL).")
        self.logs_are_private()

    def check_web(self, client):
        status, headers, body = client.request("/")
        require(status == 200 and "text/html" in headers.get("content-type", "")
                and b'<div id="root"></div>' in body, "The built web app was not served.")
        assets = re.findall(rb'(?:src|href)="(/assets/[^\"]+)"', body)
        require(bool(assets) and b"/src/main.tsx" not in body, "The image contains unbuilt web sources.")
        for asset in assets:
            status, headers, content = client.request(asset.decode())
            require(status == 200 and bool(content) and "text/html" not in headers.get("content-type", ""),
                    "A built web asset is missing or fell back to index.html.")
        status, _, body = client.request("/health/live")
        require(status == 200 and body == b"ok", "The liveness endpoint failed.")

    def run(self):
        require(sys.platform.startswith("linux"), "Docker runtime smoke tests require Linux.")
        require(shutil.which("docker") and shutil.which("openssl"), "Docker and openssl must be installed.")
        self.docker("info")
        image = self.inspect(self.image)
        require(image["Os"] == "linux" and image["Architecture"] == self.architecture,
                "The built image does not match the requested native Linux architecture.")
        config = image["Config"]
        require(config["User"] == "10001:10001", "The image must run as UID/GID 10001.")
        require(config["Entrypoint"] == ["/app/ergent-server"] and config["Cmd"] == DEFAULT_COMMAND,
                "The server image has an unexpected ENTRYPOINT or default command.")
        environment = dict(value.split("=", 1) for value in config.get("Env", []) if "=" in value)
        require(all(environment.get(key) == value for key, value in DEFAULT_ENV.items()),
                "The image is missing its documented database/listen/web ENV defaults.")
        require(config.get("StopSignal") == "SIGTERM", "The image must declare SIGTERM shutdown.")
        self.stage = "fail-closed direct TLS default"
        invalid_modes = [
            ("bare direct default", {}),
            ("direct missing certificates", {"ERGENT_ORIGIN": "https://localhost"}),
            ("direct HTTP origin", {"ERGENT_TLS_MODE": "direct", "ERGENT_ORIGIN": "http://localhost"}),
            ("proxy HTTP origin", {"ERGENT_TLS_MODE": "proxy", "ERGENT_ORIGIN": "http://localhost"}),
            ("dev-http HTTPS origin", {"ERGENT_TLS_MODE": "dev-http", "ERGENT_ORIGIN": "https://localhost"}),
            ("proxy with certificates", {"ERGENT_TLS_MODE": "proxy", "ERGENT_ORIGIN": "https://localhost",
                                         "ERGENT_TLS_CERT": "/missing.pem", "ERGENT_TLS_KEY": "/missing.key"}),
            ("dev-http with certificates", {"ERGENT_TLS_MODE": "dev-http", "ERGENT_ORIGIN": "http://localhost",
                                            "ERGENT_TLS_CERT": "/missing.pem", "ERGENT_TLS_KEY": "/missing.key"}),
        ]
        for label, environment in invalid_modes:
            args = [item for key, value in environment.items() for item in ("--env", f"{key}={value}")]
            rejected = self.docker(
                "run", "--rm", "--name", self.server, "--network", "none",
                "--read-only", "--tmpfs", "/data:uid=10001,gid=10001,mode=0700",
                *args, self.image, check=False, timeout=15,
            )
            require(rejected.returncode != 0, f"Invalid TLS configuration was accepted: {label}.")
            output = (rejected.stdout + rejected.stderr).lower()
            require(any(word in output for word in (b"direct", b"proxy", b"dev-http")),
                    f"Invalid-mode test failed for an unrelated reason: {label}.")
            require_private_output(rejected.stdout + rejected.stderr)
        print("PASS: default fails closed; invalid direct/proxy/dev-http configurations are rejected", flush=True)
        self.docker("network", "create", self.network)
        self.docker("volume", "create", self.volume)
        self.stage = "explicit dev-http startup, non-root identity, assets, and healthcheck"
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            development_port = listener.getsockname()[1]
        client = self.start(f"http://127.0.0.1:{development_port}", mode="dev-http",
                            host_port=development_port, host="127.0.0.1")
        logs = self.docker("logs", self.server)
        require("开发 HTTP 模式".encode() in logs.stdout + logs.stderr,
                "Explicit dev-http mode omitted its cleartext-development warning.")
        self.check_web(client)
        status = self.docker("exec", self.server, "cat", "/proc/1/status").stdout
        for field in (b"Uid", b"Gid"):
            values = re.search(rb"^" + field + rb":\s+(.+)$", status, re.MULTILINE)
            require(values is not None and values[1].split() == [b"10001"] * 4,
                    "The running PID 1 does not have the expected non-root identity.")
        pid1 = self.docker("exec", self.server, "cat", "/proc/1/cmdline").stdout
        require(pid1.split(b"\0")[0] == b"/app/ergent-server", "The server is not PID 1.")
        self.docker("exec", self.server, "sh", "-c", "test ! -e /app/ergent-client && test -s /data/server.db")
        require(client.json("/api/v1/auth/setup") == {"data": {"required": True}},
                "A fresh volume did not require first-account setup.")
        self.logs_are_private()
        print("PASS: explicit dev-http warning, native image, non-root PID 1, web assets, and healthcheck", flush=True)

        self.stage = "headless rejection and real docker exec -it setup-code"
        denied = self.docker("exec", self.server, "/app/ergent-server", "--database",
                             "/data/server.db", "setup-code", check=False)
        require(denied.returncode != 0, "setup-code unexpectedly succeeded without a TTY.")
        require_private_output(denied.stdout + denied.stderr)
        code = extract_code(terminal_command([
            "docker", "exec", "-it", self.server, "/app/ergent-server",
            "--database", "/data/server.db", "setup-code",
        ]))
        password = secrets.token_urlsafe(32)
        self.private_values.extend([code, password])
        self.logs_are_private()
        print("PASS: headless setup-code rejected; real operator TTY receives a private code", flush=True)

        self.stage = "HTTP localhost-origin bridge safety"
        client.get_csrf()
        body = {"username": "docker-smoke", "password": password,
                "passwordConfirmation": password, "setupCode": code}
        rejected = client.write("/api/v1/auth/setup", body, expected=403,
                                origin=client.origin)
        require(rejected["error"]["code"] == "ORIGIN_REJECTED",
                "HTTP setup over a non-loopback Docker peer was not safely rejected.")
        print("PASS: Docker bridge cannot bypass localhost-only HTTP account setup", flush=True)
        self.stage = "dev-http admin-create and protected HTTP login"
        self.docker("exec", "-i", self.server, "/app/ergent-server", "admin-create",
                    "--username", "docker-dev", input=(password + "\n").encode())
        rejected = client.write("/api/v1/auth/login", {"username": "docker-dev", "password": password},
                                expected=403, origin="http://untrusted.invalid")
        require(rejected["error"]["code"] == "ORIGIN_REJECTED", "HTTP login lost its Origin boundary.")
        saved_csrf = client.csrf
        client.csrf = "invalid"
        rejected = client.write("/api/v1/auth/login", {"username": "docker-dev", "password": password},
                                expected=403)
        require(rejected["error"]["code"] == "CSRF_INVALID", "HTTP login lost its CSRF boundary.")
        client.csrf = saved_csrf
        client.login("docker-dev", password)
        self.stop()
        self.docker("rm", self.server)
        # The proxy scenario gets a fresh disposable installation because local
        # admin-create correctly closes first-account setup permanently.
        self.docker("volume", "rm", self.volume)
        self.docker("volume", "create", self.volume)
        print("PASS: explicit dev-http CLI bootstrap and HTTP login retain Origin/CSRF protection", flush=True)

        self.stage = "HTTPS reverse proxy, setup, login, and authenticated WebSocket"
        with tempfile.TemporaryDirectory(prefix="ergent-docker-tls-") as temporary:
            folder = Path(temporary)
            make_certificates(folder)
            (folder / "nginx.conf").write_text(proxy_configuration(self.server), encoding="utf-8")
            # Docker DNS is resolved at request time, allowing the proxy to
            # start before the backend and retain its exact trusted peer IP.
            self.docker("run", "--detach", "--name", self.proxy, "--network", self.network,
                        "--publish", "127.0.0.1::443",
                        "--mount", f"type=bind,source={folder},target=/etc/nginx/test-tls,readonly",
                        "--mount", f"type=bind,source={folder / 'nginx.conf'},target=/etc/nginx/nginx.conf,readonly",
                        self.proxy_image, timeout=180)
            proxy = self.inspect(self.proxy)
            tls_port = int(proxy["NetworkSettings"]["Ports"]["443/tcp"][0]["HostPort"])
            proxy_ip = proxy["NetworkSettings"]["Networks"][self.network]["IPAddress"]
            require(bool(proxy_ip), "The test proxy has no bridge peer IP.")
            tls = ssl.create_default_context(cafile=str(folder / "cert.pem"))
            https = Client(tls_port, tls=tls)
            self.start(https.origin, trusted_proxy=proxy_ip)
            self.wait_ready(https)
            self.check_web(https)
            proxy_code = extract_code(terminal_command([
                "docker", "exec", "-it", self.server, "/app/ergent-server", "setup-code",
            ]))
            self.private_values.append(proxy_code)
            body["setupCode"] = proxy_code
            https.get_csrf()
            require(https.write("/api/v1/auth/setup", body) == {"data": {"created": True}},
                    "HTTPS first-account setup did not complete.")
            require(https.json("/api/v1/auth/setup") == {"data": {"required": False}},
                    "The completed installation remained open for setup.")
            https.json("/api/v1/auth/me", expected=401)
            websocket(https, authenticated=False, expected=401)
            https.login("docker-smoke", password)
            websocket(https)
            self.logs_are_private()
            proxy_logs = self.docker("logs", self.proxy)
            require_private_output(proxy_logs.stdout + proxy_logs.stderr, self.private_values)
            print("PASS: certificate-verified HTTPS setup/login, Secure cookies, authenticated WebSocket", flush=True)

            self.stage = "trusted-proxy forwarding and SIGTERM with an active WebSocket"
            failed_login = Client(tls_port, tls=tls)
            failed_login.get_csrf()
            wrong_password = secrets.token_urlsafe(32)
            self.private_values.append(wrong_password)
            forged_ip = "203.0.113.123"
            status, headers, _ = failed_login.request(
                "/api/v1/auth/login", method="POST",
                data={"username": "docker-smoke", "password": wrong_password},
                headers={"Origin": https.origin, "X-CSRF-Token": failed_login.csrf,
                         "X-Real-IP": forged_ip, "X-Forwarded-For": forged_ip},
            )
            require(status == 401, "The spoofed-header invalid login was not rejected.")
            real_ip = headers.get("x-smoke-proxy-peer")
            require(real_ip and real_ip not in (forged_ip, proxy_ip),
                    "The test proxy did not identify a distinct real client IP.")
            websocket(https, during_open=self.stop)
            # Read only this stopped, disposable fixture DB, including WAL if
            # present. Never copy it into CI logs or uploaded artifacts.
            snapshot = folder / "data"
            snapshot.mkdir()
            self.docker("cp", f"{self.server}:/data/.", str(snapshot))
            with sqlite3.connect(snapshot / "server.db") as database:
                rows = database.execute("SELECT ip, failures FROM login_ip_limits").fetchall()
            require(rows == [(real_ip, 1)],
                    "Login accounting did not use the overwritten real client IP from the trusted proxy.")
            print("PASS: trusted proxy rejects forged client IP; SIGTERM closes an active WebSocket", flush=True)

            self.stage = "named-volume persistence after container replacement"
            # Recreate the backend, rather than only restart it, to prove the
            # account is in the named volume instead of a container writable layer.
            self.docker("rm", self.server)
            self.start(https.origin, trusted_proxy=proxy_ip)
            fresh = Client(tls_port, tls=tls)
            self.wait_ready(fresh)
            require(fresh.json("/api/v1/auth/setup") == {"data": {"required": False}},
                    "First-account setup reopened after container replacement.")
            fresh.login("docker-smoke", password)
            websocket(fresh)
            denied = self.docker("exec", self.server, "/app/ergent-server", "--database",
                                 "/data/server.db", "setup-code", check=False)
            require(denied.returncode != 0, "Completed setup could unexpectedly issue another code.")
            require_private_output(denied.stdout + denied.stderr, self.private_values)
            self.stop()
            print("PASS: clean SIGTERM exit and account/login persistence after container replacement", flush=True)
            self.stage = "ENV-only native HTTPS setup/login and persistence"
            self.direct_tls(folder)

    def direct_tls(self, certificates):
        """Fresh-volume account setup with native TLS and ENV-only configuration."""
        fixture = SmokeTest(self.image, self.proxy_image, self.architecture, self.agent_binary)
        tls_dir = certificates / "direct"
        tls_dir.mkdir(mode=0o755)
        for name in ("cert.pem", "key.pem"):
            shutil.copyfile(certificates / name, tls_dir / name)
            # Disposable localhost test material only; non-root UID 10001 must
            # read both mounted files. No production private key is involved.
            (tls_dir / name).chmod(0o644)
        tls = ssl.create_default_context(cafile=str(tls_dir / "cert.pem"))
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        origin = f"https://127.0.0.1:{port}"
        agent = None
        try:
            fixture.docker("network", "create", fixture.network)
            fixture.docker("volume", "create", fixture.volume)
            client = fixture.start(origin, tls=tls, tls_dir=tls_dir, host_port=port, host="127.0.0.1")
            fixture.check_web(client)
            require(client.json("/api/v1/auth/setup") == {"data": {"required": True}},
                    "A fresh native-TLS installation did not require setup.")
            code = extract_code(terminal_command([
                "docker", "exec", "-it", fixture.server, "/app/ergent-server", "setup-code",
            ]))
            password = secrets.token_urlsafe(32)
            fixture.private_values.extend([code, password])
            client.get_csrf()
            result = client.write("/api/v1/auth/setup", {
                "username": "direct-tls-smoke", "password": password,
                "passwordConfirmation": password, "setupCode": code,
            })
            require(result == {"data": {"created": True}}, "Native-TLS account setup failed.")
            websocket(client, authenticated=False, expected=401)
            client.login("direct-tls-smoke", password)
            if self.agent_binary:
                agent, machine_id = fixture.start_native_agent(client, certificates)
            else:
                print("SKIP: native Agent integration (pass --agent-binary to include it)", flush=True)
            websocket(client, during_open=fixture.stop)
            fixture.docker("rm", fixture.server)
            restarted = fixture.start(origin, tls=tls, tls_dir=tls_dir, host_port=port, host="127.0.0.1")
            require(restarted.json("/api/v1/auth/setup") == {"data": {"required": False}},
                    "Native-TLS account setup reopened after container replacement.")
            restarted.login("direct-tls-smoke", password)
            if agent is not None:
                fixture.wait_native_agent(restarted, agent, machine_id)
                print("PASS: native Agent HTTPS enrollment, trusted WSS inventory, and reconnect after replacement", flush=True)
            websocket(restarted)
            fixture.stop()
            print("PASS: ENV-only native HTTPS, certificate validation, setup/login, WebSocket, and persistence", flush=True)
        finally:
            try:
                if agent is not None:
                    fixture.stop_native_agent(agent)
            finally:
                fixture.cleanup()

    def start_native_agent(self, client, folder):
        token = client.write("/api/v1/enrollments", {})["data"]["token"]
        self.private_values.append(token)
        binary = str(self.agent_binary.resolve())
        require(self.agent_binary.is_file(), "The requested native Agent binary is missing.")
        rejected_config = folder / "untrusted-agent.json"
        rejected = command([
            binary, "--config", str(rejected_config), "enroll", "--server", client.origin,
            "--name", "Untrusted Docker probe",
        ], input=(token + "\n").encode(), check=False, timeout=30)
        require(rejected.returncode != 0 and not rejected_config.exists(),
                "The native Agent accepted a private-CA server without explicit trust.")
        require_private_output(rejected.stdout + rejected.stderr, self.private_values)
        config = folder / "native-agent.json"
        enrolled = command([
            binary, "--config", str(config), "--ca-cert", str(folder / "ca.pem"),
            "enroll", "--server", client.origin, "--name", "Docker TLS probe",
        ], input=(token + "\n").encode(), timeout=30)
        require_private_output(enrolled.stdout + enrolled.stderr, self.private_values)
        data = json.loads(config.read_text())
        require(Path(data["caCert"]) == (folder / "ca.pem").resolve(),
                "Agent enrollment did not persist its explicit CA trust path.")
        require(config.stat().st_mode & 0o077 == 0, "The Agent credential file is readable by other users.")
        self.private_values.append(data["secret"])
        child = subprocess.Popen(
            [binary, "--config", str(config), "run"], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, cwd=folder,
        )
        try:
            self.wait_native_agent(client, child, data["machineId"])
        except Exception:
            self.stop_native_agent(child)
            raise
        return child, data["machineId"]

    def wait_native_agent(self, client, child, machine_id):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            require(child.poll() is None, "The native Agent exited before its authenticated inventory was ready.")
            machines = client.json("/api/v1/machines")["data"]["items"]
            if any(machine["id"] == machine_id and machine["online"] for machine in machines):
                return
            time.sleep(0.5)
        raise SmokeFailure("Native Agent WSS enrollment/inventory did not become online.")

    def stop_native_agent(self, child):
        if child.poll() is None:
            child.send_signal(signal.SIGINT)
        try:
            stdout, stderr = child.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise SmokeFailure("The native Agent did not stop after SIGINT.") from None
        require(child.returncode == 0, "The native Agent did not exit cleanly.")
        require_private_output(stdout + stderr, self.private_values)

    def cleanup(self):
        # Only remove this run's random names. Never dump inspect or docker logs:
        # terminal bootstrap codes must not become CI artifacts on failure.
        for name in (self.proxy, self.server):
            self.docker("rm", "--force", name, check=False)
        self.docker("volume", "rm", self.volume, check=False)
        self.docker("network", "rm", self.network, check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="ergent-server:smoke")
    parser.add_argument("--proxy-image", default="nginx:stable-alpine")
    parser.add_argument("--architecture", choices=("amd64", "arm64"), required=True)
    parser.add_argument("--agent-binary", type=Path, help="Native Agent binary for real HTTPS enrollment/WSS")
    args = parser.parse_args()
    smoke = SmokeTest(args.image, args.proxy_image, args.architecture, args.agent_binary)
    failed = False
    try:
        smoke.run()
    except SmokeFailure as error:
        print(f"FAIL ({smoke.stage}): {error}", file=sys.stderr)
        failed = True
    except Exception as error:
        # Exception repr/str can include HTTP payloads or subprocess output.
        print(f"FAIL ({smoke.stage}): {type(error).__name__}; details withheld to protect test credentials.",
              file=sys.stderr)
        failed = True
    finally:
        if shutil.which("docker"):
            try:
                smoke.cleanup()
            except Exception:
                print("FAIL: cleanup failed; remove this run's ergent-smoke-* resources manually.", file=sys.stderr)
                failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
