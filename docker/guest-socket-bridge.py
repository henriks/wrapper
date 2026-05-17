#!/usr/bin/env python3
"""Forward a guest TCP listener to the local Docker Unix socket."""

from __future__ import annotations

import argparse
import selectors
import socket
import sys
import threading
import time


DEFAULT_MAX_SESSIONS = 64
DEFAULT_IO_TIMEOUT_SECONDS = 30.0
DEFAULT_CONNECT_TIMEOUT_SECONDS = 5.0
CONNECT_RETRY_INTERVAL_SECONDS = 0.05


def log(msg: str) -> None:
    print(f"agentvm-socket-bridge: {msg}", flush=True)


def proxy_bidirectional(
    left: socket.socket,
    right: socket.socket,
    left_name: str = "left",
    right_name: str = "right",
    io_timeout: float = DEFAULT_IO_TIMEOUT_SECONDS,
) -> None:
    sel = selectors.DefaultSelector()
    sel.register(left, selectors.EVENT_READ, right)
    sel.register(right, selectors.EVENT_READ, left)
    sockets = (left, right)
    closed_read: set[socket.socket] = set()
    names = {left: left_name, right: right_name}
    relayed = {left_name: 0, right_name: 0}
    timeout = False

    for sock in sockets:
        sock.settimeout(io_timeout)

    try:
        while len(closed_read) < 2:
            events = sel.select(io_timeout)
            if not events:
                timeout = True
                log(f"closing idle bridge session after {io_timeout:.1f}s")
                break
            for key, _ in events:
                src = key.fileobj
                if src in closed_read:
                    continue
                dst = key.data
                try:
                    data = src.recv(65536)
                except (ConnectionResetError, TimeoutError, socket.timeout):
                    log(f"peer read failed on {names[src]}")
                    data = b""
                if not data:
                    closed_read.add(src)
                    try:
                        sel.unregister(src)
                    except Exception:
                        pass
                    try:
                        dst.shutdown(socket.SHUT_WR)
                    except OSError:
                        pass
                    continue
                try:
                    dst.sendall(data)
                except (BrokenPipeError, ConnectionResetError, TimeoutError, socket.timeout, OSError) as exc:
                    log(f"closing bridge session after write failure {names[src]} -> {names[dst]}: {exc}")
                    timeout = True
                    return
                relayed[names[src]] += len(data)
    finally:
        log(
            "bridge session closed "
            f"client_to_docker={relayed.get(left_name, 0)} "
            f"docker_to_client={relayed.get(right_name, 0)} "
            f"timed_out={str(timeout).lower()}"
        )
        for sock in sockets:
            try:
                sel.unregister(sock)
            except Exception:
                pass
            try:
                sock.close()
            except OSError:
                pass


def connect_docker_socket(docker_sock: str, connect_timeout: float) -> socket.socket:
    deadline = time.monotonic() + connect_timeout
    last_error: OSError | None = None
    while time.monotonic() < deadline:
        upstream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        upstream.settimeout(connect_timeout)
        try:
            upstream.connect(docker_sock)
            return upstream
        except OSError as exc:
            last_error = exc
            upstream.close()
            time.sleep(CONNECT_RETRY_INTERVAL_SECONDS)
    if last_error is None:
        raise TimeoutError(f"timed out connecting to {docker_sock}")
    raise last_error


def handle_client(
    client: socket.socket,
    docker_sock: str,
    session_sem: threading.BoundedSemaphore | None = None,
    connect_timeout: float = DEFAULT_CONNECT_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_IO_TIMEOUT_SECONDS,
) -> None:
    upstream: socket.socket | None = None
    try:
        log(f"accepted client, connecting to {docker_sock}")
        upstream = connect_docker_socket(docker_sock, connect_timeout)
        log("docker socket connection established")
    except OSError as exc:
        print(
            f"error: failed to connect to {docker_sock}: {exc}",
            file=sys.stderr,
            flush=True,
        )
        client.close()
        if upstream is not None:
            upstream.close()
        if session_sem is not None:
            session_sem.release()
        return

    try:
        proxy_bidirectional(client, upstream, "client", "docker", io_timeout)
    finally:
        if session_sem is not None:
            session_sem.release()


def start_client_thread(
    client: socket.socket,
    docker_sock: str,
    session_sem: threading.BoundedSemaphore,
    connect_timeout: float = DEFAULT_CONNECT_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_IO_TIMEOUT_SECONDS,
) -> bool:
    if not session_sem.acquire(blocking=False):
        log("rejecting Docker bridge client: session limit reached")
        client.close()
        return False
    thread = threading.Thread(
        target=handle_client,
        args=(client, docker_sock, session_sem, connect_timeout, io_timeout),
        daemon=True,
    )
    thread.start()
    return True


def serve_tcp(
    tcp_host: str,
    tcp_port: int,
    docker_sock: str,
    max_sessions: int = DEFAULT_MAX_SESSIONS,
    connect_timeout: float = DEFAULT_CONNECT_TIMEOUT_SECONDS,
    io_timeout: float = DEFAULT_IO_TIMEOUT_SECONDS,
) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((tcp_host, tcp_port))
    listener.listen(max_sessions)
    log(f"listening on tcp {tcp_host}:{tcp_port} max_sessions={max_sessions}")
    session_sem = threading.BoundedSemaphore(max_sessions)

    while True:
        client, _ = listener.accept()
        start_client_thread(client, docker_sock, session_sem, connect_timeout, io_timeout)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Forward a guest TCP listener to Docker's Unix socket.",
    )
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--tcp-host", default="0.0.0.0")
    parser.add_argument("--docker-sock", required=True)
    parser.add_argument("--max-sessions", type=int, default=DEFAULT_MAX_SESSIONS)
    parser.add_argument("--connect-timeout", type=float, default=DEFAULT_CONNECT_TIMEOUT_SECONDS)
    parser.add_argument("--io-timeout", type=float, default=DEFAULT_IO_TIMEOUT_SECONDS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    serve_tcp(
        args.tcp_host,
        args.tcp_port,
        args.docker_sock,
        args.max_sessions,
        args.connect_timeout,
        args.io_timeout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
