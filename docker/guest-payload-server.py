#!/usr/bin/env python3
"""Run one guest payload at a time and stream stdio over a framed TCP protocol."""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import pty
import signal
import socket
import struct
import subprocess
import termios
import threading


FRAME_HEADER = struct.Struct("!cI")


def log(msg: str) -> None:
    print(f"agentvm-payload-server: {msg}", flush=True)


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
    payload = recv_exact(sock, length) if length else b""
    return frame_type, payload


def send_frame(sock: socket.socket, frame_type: bytes, payload: bytes = b"",
               lock: threading.Lock | None = None) -> None:
    frame = FRAME_HEADER.pack(frame_type, len(payload)) + payload
    if lock is None:
        sock.sendall(frame)
        return
    with lock:
        sock.sendall(frame)


def set_winsize(fd: int, rows: int, cols: int) -> None:
    packed = struct.pack("HHHH", rows, cols, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)


def run_payload(conn: socket.socket, request: dict[str, object]) -> None:
    script = str(request.get("script") or "")
    if not script:
        raise ValueError("payload request is missing script")

    cwd = str(request.get("cwd") or "/")
    env = dict(os.environ)
    env.update({str(k): str(v) for k, v in dict(request.get("env") or {}).items()})
    rows = int(request.get("rows") or 24)
    cols = int(request.get("cols") or 80)

    master_fd, slave_fd = pty.openpty()
    set_winsize(master_fd, rows, cols)

    proc = subprocess.Popen(
        ["/bin/sh", "-c", script],
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        cwd=cwd,
        env=env,
        preexec_fn=os.setsid,
        close_fds=True,
    )
    os.close(slave_fd)
    send_lock = threading.Lock()
    finished = threading.Event()

    def output_loop() -> None:
        try:
            while True:
                try:
                    chunk = os.read(master_fd, 65536)
                except OSError as exc:
                    if exc.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                send_frame(conn, b"O", chunk, send_lock)
        except OSError as exc:
            log(f"output loop stopped: {exc}")

    def wait_loop() -> None:
        exit_code = proc.wait()
        payload = json.dumps({"exit_code": exit_code}).encode("utf-8")
        try:
            send_frame(conn, b"X", payload, send_lock)
        except OSError:
            pass
        finally:
            finished.set()

    output_thread = threading.Thread(target=output_loop, daemon=True)
    wait_thread = threading.Thread(target=wait_loop, daemon=True)
    output_thread.start()
    wait_thread.start()

    try:
        while not finished.is_set():
            frame_type, payload = recv_frame(conn)
            if frame_type == b"I":
                if payload:
                    os.write(master_fd, payload)
                continue
            if frame_type == b"W":
                dims = json.loads(payload.decode("utf-8"))
                set_winsize(master_fd, int(dims["rows"]), int(dims["cols"]))
                continue
            if frame_type == b"S":
                sig = int(json.loads(payload.decode("utf-8"))["signal"])
                try:
                    os.killpg(proc.pid, sig)
                except ProcessLookupError:
                    pass
                continue
    except EOFError:
        log("client disconnected during payload execution")
    finally:
        if proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                proc.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait(timeout=2.0)
        try:
            os.close(master_fd)
        except OSError:
            pass
        finished.set()


def handle_client(conn: socket.socket, session_lock: threading.Lock) -> None:
    with conn:
        frame_type, payload = recv_frame(conn)
        if frame_type == b"P":
            send_frame(conn, b"K", b"ok")
            return
        if frame_type != b"R":
            send_frame(conn, b"F", b"unexpected initial frame")
            return
        request = json.loads(payload.decode("utf-8"))
        if not session_lock.acquire(blocking=False):
            send_frame(conn, b"F", b"payload session already active")
            return
        try:
            run_payload(conn, request)
        except Exception as exc:  # noqa: BLE001
            log(f"payload request failed: {exc}")
            try:
                send_frame(conn, b"F", str(exc).encode("utf-8"))
            except OSError:
                pass
        finally:
            session_lock.release()


def serve_tcp(tcp_host: str, tcp_port: int) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((tcp_host, tcp_port))
    listener.listen()
    session_lock = threading.Lock()
    log(f"listening on tcp {tcp_host}:{tcp_port}")

    while True:
        conn, _ = listener.accept()
        thread = threading.Thread(
            target=handle_client,
            args=(conn, session_lock),
            daemon=True,
        )
        thread.start()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run one payload at a time and stream stdio over TCP.",
    )
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--tcp-host", default="0.0.0.0")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    serve_tcp(args.tcp_host, args.tcp_port)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
