#!/usr/bin/env python3
"""Start a local or explicit LAN preview; credentials stay in .local/dev/login.json."""
import argparse
import http.cookiejar
import ipaddress
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def save_private(path, value):
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(fd, "w") as stream:
        json.dump(value, stream, indent=2)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=7777)
    parser.add_argument("--lan", metavar="IP", help="同时开放指定私有 IPv4 局域网地址")
    parser.add_argument("--allow-any-origin", action="store_true", help="开发模式允许任意浏览器 Origin，仍需登录和 CSRF token")
    parser.add_argument("--server-only", action="store_true", help="仅启动网页服务，保留已经单独运行的 Agent")
    args = parser.parse_args()
    origin = f"http://127.0.0.1:{args.port}"
    public_origin = origin
    if args.lan:
        address = ipaddress.IPv4Address(args.lan)
        networks = [ipaddress.ip_network(n) for n in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")]
        if not any(address in network for network in networks):
            parser.error("--lan 需要 10.x、172.16–31.x 或 192.168.x 私有 IPv4 地址")
        public_origin = f"http://{address}:{args.port}"
    folder = ROOT / ".local/dev"
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    suffix = ".exe" if os.name == "nt" else ""
    server = ROOT / f"target/debug/ergent-server{suffix}"
    agent = ROOT / f"target/debug/ergent-agent{suffix}"
    if not server.exists() or not agent.exists() or not (ROOT / "apps/web/dist/index.html").exists():
        raise SystemExit("先执行 sh scripts/cargo.sh build --workspace 和 sh scripts/pnpm.sh build")
    database = folder / "server.db"
    login_file = folder / "login.json"
    if not login_file.exists():
        if database.exists():
            raise SystemExit("已有开发数据库，但缺少 login.json；请恢复该文件，避免覆盖已有账号。")
        login = {"username": "admin", "password": secrets.token_urlsafe(24)}
        subprocess.run([server, "--database", database, "admin-create"], input=login["password"], text=True, check=True)
        save_private(login_file, login)
    login = json.loads(login_file.read_text())
    children = []

    def stop(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def api(path, payload=None, csrf=None):
        headers = {"Origin": origin, "Content-Type": "application/json"}
        if csrf:
            headers["X-CSRF-Token"] = csrf
        data = None if payload is None else json.dumps(payload).encode()
        with opener.open(urllib.request.Request(origin + "/api/v1" + path, data=data, headers=headers), timeout=5) as response:
            return json.load(response)["data"]

    try:
        server_args = [server, "--database", database, "run", "--listen", f"{'0.0.0.0' if args.lan or args.allow_any_origin else '127.0.0.1'}:{args.port}", "--origin", public_origin, "--web", ROOT / "apps/web/dist"]
        if args.lan:
            server_args += ["--allow-insecure-lan", "--additional-origin", origin]
        if args.allow_any_origin:
            server_args += ["--allow-any-origin"]
        server_process = subprocess.Popen(server_args, cwd=ROOT)
        children.append(server_process)
        for _ in range(100):
            if server_process.poll() is not None:
                raise RuntimeError("Server 启动失败，请检查端口和日志")
            try:
                csrf = api("/auth/csrf")["csrfToken"]
                break
            except urllib.error.URLError:
                time.sleep(0.1)
        else:
            raise RuntimeError("Server 启动超时")
        csrf = api("/auth/login", login, csrf)["csrfToken"]
        config = folder / "agent.json"
        if not config.exists():
            token = api("/enrollments", {}, csrf)["token"]
            subprocess.run([agent, "--config", config, "enroll", "--server", origin, "--name", "Local development"], input=token, text=True, check=True, cwd=ROOT)
        elif json.loads(config.read_text())["server"] != origin:
            raise RuntimeError("已有 Agent 配置使用其他端口；请使用原端口")
        if not args.server_only:
            children.append(subprocess.Popen([agent, "--config", config, "run"], cwd=ROOT))
        stop_message = "Ctrl-C 只停止网页服务，已有 Agent 继续运行。" if args.server_only else "Ctrl-C 停止服务和 Agent（会结束本次 Agent 的终端）。"
        print(f"\nErgent preview: {public_origin}\n本机地址：{origin}\n来源校验：{'允许任意来源' if args.allow_any_origin else '精确白名单'}\n登录凭据：{login_file}\n{stop_message}\n", flush=True)
        while True:
            if any(child.poll() is not None for child in children):
                raise RuntimeError("子进程已退出，请检查日志")
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.send_signal(signal.SIGINT if os.name != "nt" else signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


if __name__ == "__main__":
    main()
