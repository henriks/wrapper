#!/usr/bin/env python3
"""Run a primary guest payload and bounded concurrent diagnostic payloads."""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import itertools
import pty
import select
import signal
import socket
import struct
import subprocess
import termios
import threading
import time


FRAME_HEADER = struct.Struct("!cI")
MAX_FRAME_PAYLOAD = 16 * 1024 * 1024
DEFAULT_DIAGNOSTIC_TIMEOUT_SECONDS = 10.0
MAX_DIAGNOSTIC_TIMEOUT_SECONDS = 60.0
DEFAULT_DIAGNOSTIC_OUTPUT_BYTES = 1024 * 1024
MAX_DIAGNOSTIC_OUTPUT_BYTES = 4 * 1024 * 1024
DEFAULT_INITIAL_FRAME_TIMEOUT_SECONDS = 10.0
DEFAULT_SESSION_IO_TIMEOUT_SECONDS = 30.0
DEFAULT_MAX_CLIENTS = 16
DEFAULT_MAX_DIAGNOSTIC_SESSIONS = 4


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
    if length > MAX_FRAME_PAYLOAD:
        raise ValueError(f"payload frame too large: {length} > {MAX_FRAME_PAYLOAD}")
    payload = recv_exact(sock, length) if length else b""
    return frame_type, payload


def send_frame(sock: socket.socket, frame_type: bytes, payload: bytes = b"",
               lock: threading.Lock | None = None) -> None:
    if len(payload) > MAX_FRAME_PAYLOAD:
        raise ValueError(f"payload frame too large: {len(payload)} > {MAX_FRAME_PAYLOAD}")
    frame = FRAME_HEADER.pack(frame_type, len(payload)) + payload
    if lock is None:
        sock.sendall(frame)
        return
    with lock:
        sock.sendall(frame)


def terminate_process_group(proc: subprocess.Popen[object], grace_seconds: float = 2.0) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait(timeout=grace_seconds)


def set_winsize(fd: int, rows: int, cols: int) -> None:
    packed = struct.pack("HHHH", rows, cols, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)


def payload_identity(env: dict[str, str]) -> tuple[int | None, int | None]:
    uid = env.get("AGENTVM_UID")
    gid = env.get("AGENTVM_GID")
    if uid is None and gid is None:
        return None, None
    if uid is None or gid is None:
        raise ValueError("payload identity requires both AGENTVM_UID and AGENTVM_GID")
    return int(uid), int(gid)


def ensure_home(env: dict[str, str], uid: int | None, gid: int | None) -> None:
    home = env.get("HOME")
    if not home or not os.path.isabs(home):
        return
    os.makedirs(home, exist_ok=True)
    if uid is None or gid is None:
        return
    home_stat = os.stat(home)
    if home_stat.st_uid != uid or home_stat.st_gid != gid:
        os.chown(home, uid, gid)


def payload_preexec(uid: int | None, gid: int | None):
    def preexec() -> None:
        os.setsid()
        if os.isatty(0):
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
            os.tcsetpgrp(0, os.getpgrp())
        if uid is None or gid is None:
            return
        if hasattr(os, "setgroups"):
            os.setgroups([gid])
        os.setgid(gid)
        os.setuid(uid)

    return preexec


def run_payload(conn: socket.socket, request: dict[str, object]) -> None:
    script = str(request.get("script") or "")
    if not script:
        raise ValueError("payload request is missing script")

    cwd = str(request.get("cwd") or "/")
    env = dict(os.environ)
    env.update({str(k): str(v) for k, v in dict(request.get("env") or {}).items()})
    uid, gid = payload_identity(env)
    ensure_home(env, uid, gid)
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
        preexec_fn=payload_preexec(uid, gid),
        close_fds=True,
    )
    os.close(slave_fd)
    send_lock = threading.Lock()
    finished = threading.Event()
    client_failed = threading.Event()

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
            client_failed.set()
            finished.set()
            log(f"output loop stopped: {exc}")

    def wait_loop() -> None:
        exit_code = proc.wait()
        if client_failed.is_set():
            finished.set()
            return
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
            readable, _, _ = select.select([conn], [], [], 0.1)
            if not readable:
                continue
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
    except (EOFError, TimeoutError, socket.timeout):
        log("client disconnected or timed out during payload execution")
    finally:
        terminate_process_group(proc)
        try:
            os.close(master_fd)
        except OSError:
            pass
        finished.set()


def bounded_float(value: object, default: float, maximum: float) -> float:
    if value is None:
        return default
    parsed = float(value)
    if parsed <= 0:
        raise ValueError("diagnostic timeout must be positive")
    return min(parsed, maximum)


def bounded_int(value: object, default: int, maximum: int) -> int:
    if value is None:
        return default
    parsed = int(value)
    if parsed <= 0:
        raise ValueError("diagnostic output limit must be positive")
    return min(parsed, maximum)


def run_diagnostic(conn: socket.socket, request: dict[str, object], session_id: int) -> None:
    script = str(request.get("script") or "")
    if not script:
        raise ValueError("diagnostic request is missing script")

    cwd = str(request.get("cwd") or "/")
    env = dict(os.environ)
    env.update({str(k): str(v) for k, v in dict(request.get("env") or {}).items()})
    uid, gid = payload_identity(env)
    ensure_home(env, uid, gid)
    timeout_seconds = bounded_float(
        request.get("timeout_seconds"),
        DEFAULT_DIAGNOSTIC_TIMEOUT_SECONDS,
        MAX_DIAGNOSTIC_TIMEOUT_SECONDS,
    )
    max_output_bytes = bounded_int(
        request.get("max_output_bytes"),
        DEFAULT_DIAGNOSTIC_OUTPUT_BYTES,
        MAX_DIAGNOSTIC_OUTPUT_BYTES,
    )

    log(f"diagnostic session {session_id} starting timeout={timeout_seconds}s max_output={max_output_bytes}")
    proc = subprocess.Popen(
        ["/bin/sh", "-c", script],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        cwd=cwd,
        env=env,
        preexec_fn=payload_preexec(uid, gid),
        close_fds=True,
    )
    if proc.stdout is None:
        raise RuntimeError("diagnostic stdout pipe was not created")

    deadline = time.monotonic() + timeout_seconds
    sent = 0
    timed_out = False
    truncated = False
    fd = proc.stdout.fileno()
    try:
        try:
            while True:
                now = time.monotonic()
                if now >= deadline and proc.poll() is None:
                    timed_out = True
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                remaining = max(0.0, deadline - now) if proc.poll() is None else 0.0
                readable, _, _ = select.select([fd], [], [], min(0.1, remaining))
                if readable:
                    capacity = max_output_bytes - sent
                    if capacity <= 0:
                        truncated = True
                        if proc.poll() is None:
                            try:
                                os.killpg(proc.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        continue
                    chunk = os.read(fd, min(65536, capacity))
                    if chunk:
                        sent += len(chunk)
                        send_frame(conn, b"O", chunk)
                        if sent >= max_output_bytes and proc.poll() is None:
                            truncated = True
                            try:
                                os.killpg(proc.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        continue
                if proc.poll() is not None:
                    while True:
                        chunk = os.read(fd, min(65536, max(0, max_output_bytes - sent)))
                        if not chunk:
                            break
                        sent += len(chunk)
                        send_frame(conn, b"O", chunk)
                        if sent >= max_output_bytes:
                            truncated = True
                            break
                    break
        except OSError:
            terminate_process_group(proc)
            raise
    finally:
        try:
            proc.stdout.close()
        except OSError:
            pass

    exit_code = proc.wait()
    if timed_out:
        exit_code = 124
        send_frame(conn, b"O", b"\nagentvm diagnostic timed out\n")
    elif truncated:
        exit_code = 125
        send_frame(conn, b"O", b"\nagentvm diagnostic output limit exceeded\n")
    payload = json.dumps({
        "exit_code": exit_code,
        "diagnostic": True,
        "timed_out": timed_out,
        "truncated": truncated,
    }).encode("utf-8")
    send_frame(conn, b"X", payload)
    log(f"diagnostic session {session_id} exited code={exit_code} timed_out={timed_out} truncated={truncated}")


def handle_client(
    conn: socket.socket,
    session_lock: threading.Lock,
    diagnostic_sem: threading.BoundedSemaphore | None = None,
    session_ids: itertools.count | None = None,
    initial_timeout: float = DEFAULT_INITIAL_FRAME_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_SESSION_IO_TIMEOUT_SECONDS,
) -> None:
    if diagnostic_sem is None:
        diagnostic_sem = threading.BoundedSemaphore(DEFAULT_MAX_DIAGNOSTIC_SESSIONS)
    if session_ids is None:
        session_ids = itertools.count(1)
    with conn:
        conn.settimeout(initial_timeout)
        try:
            frame_type, payload = recv_frame(conn)
        except (EOFError, TimeoutError, socket.timeout) as exc:
            log(f"payload client disconnected or timed out before initial frame: {exc}")
            return
        except ValueError as exc:
            log(f"payload protocol failed: {exc}")
            try:
                send_frame(conn, b"F", str(exc).encode("utf-8"))
            except OSError:
                pass
            return
        conn.settimeout(io_timeout)
        if frame_type == b"P":
            send_frame(conn, b"K", b"ok")
            return
        if frame_type not in (b"R", b"D"):
            send_frame(conn, b"F", b"unexpected initial frame")
            return
        try:
            request = json.loads(payload.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            send_frame(conn, b"F", f"invalid payload request JSON: {exc}".encode("utf-8"))
            return
        if frame_type == b"D":
            if not diagnostic_sem.acquire(blocking=False):
                send_frame(conn, b"F", b"too many diagnostic sessions active")
                return
            session_id = next(session_ids)
            try:
                run_diagnostic(conn, request, session_id)
            except Exception as exc:  # noqa: BLE001
                log(f"diagnostic request failed: {exc}")
                try:
                    send_frame(conn, b"F", str(exc).encode("utf-8"))
                except OSError:
                    pass
            finally:
                diagnostic_sem.release()
            return
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


def start_client_thread(
    conn: socket.socket,
    session_lock: threading.Lock,
    diagnostic_sem: threading.BoundedSemaphore,
    session_ids: itertools.count,
    client_sem: threading.BoundedSemaphore,
    initial_timeout: float = DEFAULT_INITIAL_FRAME_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_SESSION_IO_TIMEOUT_SECONDS,
) -> bool:
    if not client_sem.acquire(blocking=False):
        log("rejecting payload client: session limit reached")
        conn.close()
        return False

    def run() -> None:
        try:
            handle_client(conn, session_lock, diagnostic_sem, session_ids, initial_timeout, io_timeout)
        finally:
            client_sem.release()

    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    return True


def serve_tcp(
    tcp_host: str,
    tcp_port: int,
    max_clients: int = DEFAULT_MAX_CLIENTS,
    max_diagnostics: int = DEFAULT_MAX_DIAGNOSTIC_SESSIONS,
    initial_timeout: float = DEFAULT_INITIAL_FRAME_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_SESSION_IO_TIMEOUT_SECONDS,
) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((tcp_host, tcp_port))
    listener.listen(max_clients)
    session_lock = threading.Lock()
    diagnostic_sem = threading.BoundedSemaphore(max_diagnostics)
    client_sem = threading.BoundedSemaphore(max_clients)
    session_ids = itertools.count(1)
    log(f"listening on tcp {tcp_host}:{tcp_port} max_clients={max_clients} max_diagnostics={max_diagnostics}")

    while True:
        conn, _ = listener.accept()
        start_client_thread(conn, session_lock, diagnostic_sem, session_ids, client_sem, initial_timeout, io_timeout)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run one primary payload and bounded diagnostic payloads over TCP.",
    )
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--tcp-host", default="0.0.0.0")
    parser.add_argument("--max-clients", type=int, default=DEFAULT_MAX_CLIENTS)
    parser.add_argument("--max-diagnostics", type=int, default=DEFAULT_MAX_DIAGNOSTIC_SESSIONS)
    parser.add_argument("--initial-timeout", type=float, default=DEFAULT_INITIAL_FRAME_TIMEOUT_SECONDS)
    parser.add_argument("--io-timeout", type=float, default=DEFAULT_SESSION_IO_TIMEOUT_SECONDS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    serve_tcp(
        args.tcp_host,
        args.tcp_port,
        args.max_clients,
        args.max_diagnostics,
        args.initial_timeout,
        args.io_timeout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
