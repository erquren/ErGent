"""Offline tests for the Docker smoke runner; no Docker daemon or registry access."""

import contextlib
import importlib.util
import io
import os
import shutil
import ssl
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "docker_smoke", Path(__file__).resolve().parents[1] / "test-docker.py",
)
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)
SECRET = "B" * 43


class CommandTests(unittest.TestCase):
    def test_failed_command_never_displays_captured_output(self):
        result = subprocess.CompletedProcess(["fixture"], 7, SECRET.encode(), SECRET.encode())
        with mock.patch.object(smoke.subprocess, "run", return_value=result):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                smoke.command(["fixture", SECRET])
        self.assertNotIn(SECRET, str(raised.exception))
        self.assertIn("exit 7", str(raised.exception))

    def test_command_timeout_never_displays_output_or_arguments(self):
        error = subprocess.TimeoutExpired(["fixture", SECRET], 1, output=SECRET.encode())
        with mock.patch.object(smoke.subprocess, "run", side_effect=error):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                smoke.command(["fixture", SECRET])
        self.assertNotIn(SECRET, str(raised.exception))

    def test_unchecked_command_returns_private_output_to_caller(self):
        result = subprocess.CompletedProcess(["fixture"], 1, SECRET.encode(), b"")
        with mock.patch.object(smoke.subprocess, "run", return_value=result):
            self.assertIs(smoke.command(["fixture"], check=False), result)

    def test_sensitive_input_is_sent_by_stdin_without_printing(self):
        program = "import sys; data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data)"
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            result = smoke.command([sys.executable, "-c", program], input=SECRET.encode())
        self.assertEqual(result.stdout, SECRET.encode())
        self.assertEqual(output.getvalue(), "")

    @unittest.skipUnless(os.name == "posix", "Controlling PTY is Unix-only")
    def test_terminal_runner_captures_real_dev_tty_without_printing(self):
        # Prove /dev/tty is really available, not merely isatty(stdout). The
        # code is hard-coded synthetic test data, never a real credential.
        program = (
            "import os; assert os.isatty(0); "
            f"f=open('/dev/tty','w'); f.write('Setup code: {SECRET}\\n'); f.close()"
        )
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            captured = smoke.terminal_command([sys.executable, "-c", program])
        self.assertEqual(smoke.extract_code(captured), SECRET)
        self.assertEqual(output.getvalue(), "")

    @unittest.skipUnless(os.name == "posix", "Controlling PTY is Unix-only")
    def test_terminal_failure_never_displays_private_output(self):
        program = f"import sys; print('{SECRET}', flush=True); sys.exit(4)"
        with self.assertRaises(smoke.SmokeFailure) as raised:
            smoke.terminal_command([sys.executable, "-c", program])
        self.assertNotIn(SECRET, str(raised.exception))

    @unittest.skipUnless(os.name == "posix", "Controlling PTY is Unix-only")
    def test_terminal_timeout_is_bounded_and_secret_safe(self):
        program = f"import time; print('{SECRET}', flush=True); time.sleep(10)"
        with self.assertRaises(smoke.SmokeFailure) as raised:
            smoke.terminal_command([sys.executable, "-c", program], timeout=0.2)
        self.assertNotIn(SECRET, str(raised.exception))
        self.assertIn("timed out", str(raised.exception))


class SecretTests(unittest.TestCase):
    def test_setup_code_accepts_terminal_crlf(self):
        output = f"\r\nSetup code: {SECRET}\r\nKeep private\r\n".encode()
        self.assertEqual(smoke.extract_code(output), SECRET)

    def test_setup_code_must_be_exactly_one_complete_token(self):
        for output in (b"", f"Setup code: {SECRET}x\n".encode(),
                       f"Setup code: {SECRET}\nSetup code: {SECRET}\n".encode()):
            with self.subTest(output_length=len(output)):
                with self.assertRaises(smoke.SmokeFailure) as raised:
                    smoke.extract_code(output)
                self.assertNotIn(SECRET, str(raised.exception))

    def test_private_log_scan_never_quotes_found_secret(self):
        for output in (f"Setup code: {SECRET}\n".encode(), f"password={SECRET}".encode()):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                smoke.require_private_output(output, [SECRET])
            self.assertNotIn(SECRET, str(raised.exception))
        smoke.require_private_output(b"First-account setup requires a console code.", [SECRET])

    def test_main_suppresses_unexpected_exception_payload(self):
        error_output = io.StringIO()
        with mock.patch.object(sys, "argv", ["test-docker.py", "--architecture", "amd64"]), \
                mock.patch.object(smoke.SmokeTest, "run", side_effect=RuntimeError(SECRET)), \
                mock.patch.object(smoke.SmokeTest, "cleanup"), \
                contextlib.redirect_stderr(error_output):
            self.assertEqual(smoke.main(), 1)
        self.assertNotIn(SECRET, error_output.getvalue())
        self.assertIn("RuntimeError", error_output.getvalue())


class LifecycleTests(unittest.TestCase):
    def fixture(self):
        fixture = smoke.SmokeTest("fixture-image", "fixture-proxy", "amd64")
        fixture.docker = mock.Mock()
        fixture.inspect = mock.Mock(return_value={
            "NetworkSettings": {"Ports": {"7777/tcp": [{"HostPort": "12345"}]}},
        })
        fixture.wait_ready = mock.Mock()
        return fixture

    def test_proxy_start_is_hardened_and_env_configured(self):
        fixture = self.fixture()
        client = fixture.start("https://localhost:443", trusted_proxy="192.0.2.2")
        args = fixture.docker.call_args.args
        self.assertIn("--read-only", args)
        self.assertIn("ALL", args)
        self.assertIn("no-new-privileges:true", args)
        self.assertIn("/tmp:rw,noexec,nosuid,size=64m", args)
        self.assertIn("ERGENT_TLS_MODE=proxy", args)
        self.assertIn("ERGENT_ORIGIN=https://localhost:443", args)
        self.assertIn("ERGENT_TRUSTED_PROXY=192.0.2.2", args)
        self.assertIn("127.0.0.1::7777", args)
        self.assertEqual(args[-1], "fixture-image")
        self.assertIsNone(client.tls)

    def test_development_http_mode_is_explicit(self):
        fixture = self.fixture()
        fixture.start("http://127.0.0.1:7777", mode="dev-http", host="127.0.0.1")
        args = fixture.docker.call_args.args
        self.assertIn("ERGENT_TLS_MODE=dev-http", args)
        self.assertNotIn("ERGENT_TLS_MODE=proxy", args)

    def test_direct_start_mounts_certificates_and_verifies_https(self):
        fixture = self.fixture()
        client = fixture.start("https://localhost:12345", tls=mock.sentinel.tls,
                               tls_dir=Path("/fixture/certs"), host_port=12345)
        args = fixture.docker.call_args.args
        self.assertIn("ERGENT_TLS_MODE=direct", args)
        self.assertIn("ERGENT_TLS_CERT=/test-tls/cert.pem", args)
        self.assertIn("ERGENT_TLS_KEY=/test-tls/key.pem", args)
        self.assertIn(f"type=bind,source={Path('/fixture/certs')},target=/test-tls,readonly", args)
        self.assertIn("127.0.0.1:12345:7777", args)
        self.assertIs(client.tls, mock.sentinel.tls)
        fixture.wait_ready.assert_called_once_with(client)

    def test_stop_rejects_sigkill_fallback(self):
        fixture = self.fixture()
        fixture.logs_are_private = mock.Mock()
        fixture.inspect.return_value = {"State": {"Running": False, "ExitCode": 137, "OOMKilled": False}}
        with self.assertRaises(smoke.SmokeFailure):
            fixture.stop()

    def test_stop_requires_exit_zero_and_scans_logs(self):
        fixture = self.fixture()
        fixture.logs_are_private = mock.Mock()
        fixture.inspect.return_value = {"State": {"Running": False, "ExitCode": 0, "OOMKilled": False}}
        fixture.stop()
        self.assertEqual(fixture.logs_are_private.call_count, 2)
        self.assertIn("SIGTERM", fixture.docker.call_args.args)

    def test_cleanup_only_targets_randomly_scoped_resources(self):
        fixture = self.fixture()
        fixture.cleanup()
        self.assertEqual(fixture.docker.call_count, 4)
        for call in fixture.docker.call_args_list:
            self.assertTrue(call.args[-1].startswith("ergent-smoke-"))
            self.assertFalse(call.kwargs["check"])


class CertificateTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("openssl"), "OpenSSL is needed for the disposable certificate fixture")
    def test_private_ca_signs_non_ca_ip_san_leaf(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            smoke.make_certificates(folder)
            verified = smoke.command([
                "openssl", "verify", "-CAfile", str(folder / "ca.pem"), str(folder / "leaf.pem"),
            ])
            self.assertEqual(verified.returncode, 0)
            details = smoke.command([
                "openssl", "x509", "-in", str(folder / "leaf.pem"), "-noout", "-text",
            ]).stdout
            self.assertIn(b"CA:FALSE", details)
            self.assertIn(b"IP Address:127.0.0.1", details)
            self.assertIn(b"DNS:localhost", details)
            context = ssl.create_default_context(cafile=str(folder / "cert.pem"))
            self.assertTrue(context.check_hostname)
            self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)


class ClientTests(unittest.TestCase):
    def response(self, cookie, payload=b'{}', status=200):
        response = mock.Mock()
        response.status = status
        response.getheaders.return_value = [("Set-Cookie", cookie)] if cookie else []
        response.read.return_value = payload
        connection = mock.MagicMock()
        connection.getresponse.return_value = response
        return connection

    def test_secure_cookie_preserved_without_header_logging(self):
        connection = self.response(f"ergent_session={SECRET}; HttpOnly; SameSite=Strict; Secure")
        client = smoke.Client(443, tls=mock.sentinel.context)
        with mock.patch.object(smoke.http.client, "HTTPSConnection", return_value=connection):
            self.assertEqual(client.json("/api/v1/auth/csrf"), {})
        self.assertEqual(client.cookie, f"ergent_session={SECRET}")
        connection.close.assert_called_once()

    def test_direct_ip_origin_preserves_ip_san_verification(self):
        connection = self.response(None)
        client = smoke.Client(12345, tls=mock.sentinel.context, host="127.0.0.1")
        with mock.patch.object(smoke.http.client, "HTTPSConnection", return_value=connection) as connect:
            client.request("/health/ready")
        self.assertEqual(client.origin, "https://127.0.0.1:12345")
        self.assertEqual(connect.call_args.args[0], "127.0.0.1")
        self.assertIs(connect.call_args.kwargs["context"], mock.sentinel.context)

    def test_https_rejects_cookie_without_secure_attribute(self):
        connection = self.response(f"ergent_session={SECRET}; HttpOnly; SameSite=Strict")
        client = smoke.Client(443, tls=mock.sentinel.context)
        with mock.patch.object(smoke.http.client, "HTTPSConnection", return_value=connection):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                client.request("/api/v1/auth/csrf")
        self.assertNotIn(SECRET, str(raised.exception))
        connection.close.assert_called_once()

    def test_api_status_failure_does_not_print_response_payload(self):
        client = smoke.Client(7777)
        with mock.patch.object(client, "request", return_value=(500, {}, SECRET.encode())):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                client.json("/api/v1/auth/setup")
        self.assertNotIn(SECRET, str(raised.exception))
        self.assertIn("HTTP 500", str(raised.exception))

    def test_api_invalid_json_does_not_print_response_payload(self):
        client = smoke.Client(7777)
        with mock.patch.object(client, "request", return_value=(200, {}, SECRET.encode())):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                client.json("/api/v1/auth/setup")
        self.assertNotIn(SECRET, str(raised.exception))

    def test_websocket_handshake_parser_preserves_headers(self):
        status, headers = smoke.parse_upgrade(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
            b"Sec-WebSocket-Protocol: ergent.preview.v3",
        )
        self.assertEqual(status, 101)
        self.assertEqual(headers["upgrade"], "websocket")
        self.assertEqual(headers["sec-websocket-protocol"], smoke.SUBPROTOCOL)

    def test_websocket_handshake_error_does_not_quote_data(self):
        with self.assertRaises(smoke.SmokeFailure) as raised:
            smoke.parse_upgrade(SECRET.encode())
        self.assertNotIn(SECRET, str(raised.exception))

    def test_proxy_overwrites_forwarding_headers_and_enables_websocket(self):
        config = smoke.proxy_configuration("fixture-server")
        self.assertIn('set $backend "fixture-server:7777";', config)
        self.assertIn("proxy_pass http://$backend;", config)
        self.assertIn("resolver 127.0.0.11 valid=1s;", config)
        self.assertIn("add_header X-Smoke-Proxy-Peer $remote_addr always;", config)
        self.assertIn("ssl_certificate ", config)
        self.assertIn("proxy_set_header X-Real-IP $remote_addr;", config)
        self.assertIn("proxy_set_header X-Forwarded-Proto https;", config)
        self.assertIn("proxy_http_version 1.1;", config)
        self.assertIn("proxy_set_header Upgrade $http_upgrade;", config)
        self.assertIn("access_log off;", config)


if __name__ == "__main__":
    unittest.main()
