#!/usr/bin/env python3
"""Offline tests for guest Python services and guest-init syntax."""

from __future__ import annotations

import importlib.util
import json
import socket
import struct
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FRAME_HEADER = struct.Struct("!cI")


def load_guest_module(name: str, relative: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


payload_server = load_guest_module("guest_payload_server", "docker/guest-payload-server.py")
socket_bridge = load_guest_module("guest_socket_bridge", "docker/guest-socket-bridge.py")


def send_frame(sock: socket.socket, frame_type: bytes, payload: bytes = b"") -> None:
    sock.sendall(FRAME_HEADER.pack(frame_type, len(payload)) + payload)


def recv_exact(sock: socket.socket, size: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < size:
        chunk = sock.recv(size - len(chunks))
        if not chunk:
            raise EOFError("unexpected EOF")
        chunks.extend(chunk)
    return bytes(chunks)


def recv_frame(sock: socket.socket) -> tuple[bytes, bytes]:
    frame_type, length = FRAME_HEADER.unpack(recv_exact(sock, FRAME_HEADER.size))
    return frame_type, recv_exact(sock, length) if length else b""


class GuestPayloadServerTests(unittest.TestCase):
    def socket_pair(self) -> tuple[socket.socket, socket.socket]:
        left, right = socket.socketpair()
        left.settimeout(5.0)
        right.settimeout(5.0)
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        return left, right

    def test_ping_frame_returns_ok(self) -> None:
        client, server = self.socket_pair()
        send_frame(client, b"P")

        payload_server.handle_client(server, threading.Lock())

        frame_type, payload = recv_frame(client)
        self.assertEqual((frame_type, payload), (b"K", b"ok"))

    def test_unexpected_initial_frame_returns_failure(self) -> None:
        client, server = self.socket_pair()
        send_frame(client, b"Z", b"bad")

        payload_server.handle_client(server, threading.Lock())

        frame_type, payload = recv_frame(client)
        self.assertEqual(frame_type, b"F")
        self.assertIn(b"unexpected initial frame", payload)

    def test_fragmented_initial_frame_is_reassembled(self) -> None:
        client, server = self.socket_pair()
        thread = threading.Thread(
            target=payload_server.handle_client,
            args=(server, threading.Lock()),
            daemon=True,
        )
        thread.start()
        frame = FRAME_HEADER.pack(b"P", 0)
        client.sendall(frame[:2])
        client.sendall(frame[2:])

        frame_type, payload = recv_frame(client)
        thread.join(timeout=5.0)
        self.assertFalse(thread.is_alive())
        self.assertEqual((frame_type, payload), (b"K", b"ok"))

    def test_invalid_request_json_returns_failure_frame(self) -> None:
        client, server = self.socket_pair()
        send_frame(client, b"R", b"{not-json")

        payload_server.handle_client(server, threading.Lock())

        frame_type, payload = recv_frame(client)
        self.assertEqual(frame_type, b"F")
        self.assertIn(b"invalid payload request JSON", payload)

    def test_oversized_initial_frame_returns_failure_without_payload_read(self) -> None:
        client, server = self.socket_pair()
        client.sendall(FRAME_HEADER.pack(b"R", payload_server.MAX_FRAME_PAYLOAD + 1))

        payload_server.handle_client(server, threading.Lock())

        frame_type, payload = recv_frame(client)
        self.assertEqual(frame_type, b"F")
        self.assertIn(b"payload frame too large", payload)

    def test_eof_mid_initial_frame_does_not_hang(self) -> None:
        client, server = self.socket_pair()
        thread = threading.Thread(
            target=payload_server.handle_client,
            args=(server, threading.Lock()),
            daemon=True,
        )
        thread.start()
        client.sendall(FRAME_HEADER.pack(b"R", 4) + b"ab")
        client.close()

        thread.join(timeout=5.0)
        self.assertFalse(thread.is_alive())

    def test_send_frame_rejects_oversized_payload(self) -> None:
        client, _server = self.socket_pair()
        with self.assertRaisesRegex(ValueError, "payload frame too large"):
            payload_server.send_frame(client, b"O", b"x" * (payload_server.MAX_FRAME_PAYLOAD + 1))

    def test_payload_request_streams_output_and_exit_code(self) -> None:
        client, server = self.socket_pair()
        with tempfile.TemporaryDirectory() as cwd:
            thread = threading.Thread(
                target=payload_server.handle_client,
                args=(server, threading.Lock()),
                daemon=True,
            )
            thread.start()
            request = {
                "script": "printf payload-ok",
                "cwd": cwd,
                "env": {},
                "rows": 24,
                "cols": 80,
            }
            send_frame(client, b"R", json.dumps(request).encode("utf-8"))

            output = bytearray()
            exit_code = None
            for _ in range(16):
                frame_type, payload = recv_frame(client)
                if frame_type == b"O":
                    output.extend(payload)
                elif frame_type == b"X":
                    exit_code = json.loads(payload.decode("utf-8"))["exit_code"]
                    break
                elif frame_type == b"F":
                    self.fail(f"payload failed: {payload!r}")

            client.close()
            thread.join(timeout=5.0)
            self.assertFalse(thread.is_alive())
            self.assertIn(b"payload-ok", output)
            self.assertEqual(exit_code, 0)

    def test_payload_identity_requires_uid_and_gid_together(self) -> None:
        with self.assertRaisesRegex(ValueError, "requires both"):
            payload_server.payload_identity({"AGENTVM_UID": "1000"})


class GuestSocketBridgeTests(unittest.TestCase):
    def socket_pair(self) -> tuple[socket.socket, socket.socket]:
        left, right = socket.socketpair()
        left.settimeout(5.0)
        right.settimeout(5.0)
        self.addCleanup(left.close)
        self.addCleanup(right.close)
        return left, right

    def test_proxy_bidirectional_relays_and_closes(self) -> None:
        client_side, bridge_left = self.socket_pair()
        docker_side, bridge_right = self.socket_pair()
        thread = threading.Thread(
            target=socket_bridge.proxy_bidirectional,
            args=(bridge_left, bridge_right, "client", "docker"),
            daemon=True,
        )
        thread.start()

        client_side.sendall(b"GET /version\r\n")
        self.assertEqual(docker_side.recv(1024), b"GET /version\r\n")
        docker_side.sendall(b"HTTP/1.1 200 OK\r\n")
        self.assertEqual(client_side.recv(1024), b"HTTP/1.1 200 OK\r\n")

        client_side.shutdown(socket.SHUT_WR)
        docker_side.shutdown(socket.SHUT_WR)
        thread.join(timeout=5.0)
        self.assertFalse(thread.is_alive())

    def test_proxy_bidirectional_preserves_arbitrary_binary_streams(self) -> None:
        client_side, bridge_left = self.socket_pair()
        docker_side, bridge_right = self.socket_pair()
        thread = threading.Thread(
            target=socket_bridge.proxy_bidirectional,
            args=(bridge_left, bridge_right, "client", "docker"),
            daemon=True,
        )
        thread.start()

        payload = b"\x00\xffGET /containers/json HTTP/1.1\r\n\r\n"
        client_side.sendall(payload)
        self.assertEqual(docker_side.recv(1024), payload)

        response = b"HTTP/1.1 400 Bad Request\r\n\x00malformed-ok"
        docker_side.sendall(response)
        self.assertEqual(client_side.recv(1024), response)

        client_side.shutdown(socket.SHUT_WR)
        docker_side.shutdown(socket.SHUT_WR)
        thread.join(timeout=5.0)
        self.assertFalse(thread.is_alive())

    def test_handle_client_relays_to_unix_docker_socket(self) -> None:
        client, server = self.socket_pair()
        with tempfile.TemporaryDirectory() as tmp:
            docker_sock = Path(tmp) / "docker.sock"
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            listener.bind(str(docker_sock))
            listener.listen(1)
            listener.settimeout(5.0)
            self.addCleanup(listener.close)

            bridge_thread = threading.Thread(
                target=socket_bridge.handle_client,
                args=(server, str(docker_sock)),
                daemon=True,
            )
            bridge_thread.start()
            upstream, _ = listener.accept()
            upstream.settimeout(5.0)
            self.addCleanup(upstream.close)

            client.sendall(b"GET /_ping HTTP/1.1\r\n\r\n")
            self.assertEqual(upstream.recv(1024), b"GET /_ping HTTP/1.1\r\n\r\n")
            upstream.sendall(b"OK")
            self.assertEqual(client.recv(1024), b"OK")

            client.shutdown(socket.SHUT_WR)
            upstream.shutdown(socket.SHUT_WR)
            bridge_thread.join(timeout=5.0)
            self.assertFalse(bridge_thread.is_alive())

    def test_handle_client_reconnects_for_separate_clients(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            docker_sock = Path(tmp) / "docker.sock"
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            listener.bind(str(docker_sock))
            listener.listen(2)
            listener.settimeout(5.0)
            self.addCleanup(listener.close)

            for index in range(2):
                client, server = self.socket_pair()
                bridge_thread = threading.Thread(
                    target=socket_bridge.handle_client,
                    args=(server, str(docker_sock)),
                    daemon=True,
                )
                bridge_thread.start()
                upstream, _ = listener.accept()
                upstream.settimeout(5.0)
                self.addCleanup(upstream.close)

                request = f"GET /v{index}/version HTTP/1.1\r\n\r\n".encode("ascii")
                client.sendall(request)
                self.assertEqual(upstream.recv(1024), request)
                upstream.sendall(b"{}")
                self.assertEqual(client.recv(1024), b"{}")

                client.shutdown(socket.SHUT_WR)
                upstream.shutdown(socket.SHUT_WR)
                bridge_thread.join(timeout=5.0)
                self.assertFalse(bridge_thread.is_alive())

    def test_handle_client_closes_when_docker_socket_missing(self) -> None:
        client, server = self.socket_pair()

        socket_bridge.handle_client(server, "/tmp/agentvm-missing-docker.sock")

        self.assertEqual(client.recv(1), b"")


class GuestInitTests(unittest.TestCase):
    def test_guest_init_has_valid_shell_syntax(self) -> None:
        result = subprocess.run(
            ["sh", "-n", str(ROOT / "docker/guest-init.sh")],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
