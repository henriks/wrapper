#!/usr/bin/env python3
"""Forward a guest TCP listener to the local Docker Unix socket."""

from __future__ import annotations

import argparse
import selectors
import socket
import sys
import threading


def log(msg: str) -> None:
    print(f"agentvm-socket-bridge: {msg}", flush=True)


def proxy_bidirectional(left: socket.socket, right: socket.socket,
                        left_name: str = "left", right_name: str = "right"
                        ) -> None:
    sel = selectors.DefaultSelector()
    sel.register(left, selectors.EVENT_READ, right)
    sel.register(right, selectors.EVENT_READ, left)
    sockets = (left, right)
    closed_read: set[socket.socket] = set()
    names = {left: left_name, right: right_name}

    try:
        while len(closed_read) < 2:
            events = sel.select()
            if not events:
                continue
            for key, _ in events:
                src = key.fileobj
                if src in closed_read:
                    continue
                dst = key.data
                try:
                    data = src.recv(65536)
                except ConnectionResetError:
                    log(f"peer reset connection on {names[src]}")
                    data = b""
                if not data:
                    log(
                        f"EOF on {names[src]}, "
                        f"shutting down write side of {names[dst]}"
                    )
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
                log(
                    f"relaying {len(data)} bytes "
                    f"{names[src]} -> {names[dst]}"
                )
                dst.sendall(data)
    finally:
        for sock in sockets:
            try:
                sel.unregister(sock)
            except Exception:
                pass
            try:
                sock.close()
            except OSError:
                pass


def handle_client(client: socket.socket, docker_sock: str) -> None:
    upstream: socket.socket | None = None
    try:
        log(f"accepted client, connecting to {docker_sock}")
        upstream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        upstream.connect(docker_sock)
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
        return

    proxy_bidirectional(client, upstream, "client", "docker")


def serve_tcp(tcp_host: str, tcp_port: int, docker_sock: str) -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((tcp_host, tcp_port))
    listener.listen()
    log(f"listening on tcp {tcp_host}:{tcp_port}")

    while True:
        client, _ = listener.accept()
        thread = threading.Thread(
            target=handle_client,
            args=(client, docker_sock),
            daemon=True,
        )
        thread.start()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Forward a guest TCP listener to Docker's Unix socket.",
    )
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--tcp-host", default="0.0.0.0")
    parser.add_argument("--docker-sock", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    serve_tcp(args.tcp_host, args.tcp_port, args.docker_sock)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
