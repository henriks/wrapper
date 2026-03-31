#!/usr/bin/env python3
"""Forward a guest vsock listener to the local Docker Unix socket."""

from __future__ import annotations

import argparse
import selectors
import socket
import sys
import threading


def log(msg: str) -> None:
    print(f"agentvm-vsock-bridge: {msg}", flush=True)


def proxy_bidirectional(left: socket.socket, right: socket.socket) -> None:
    sel = selectors.DefaultSelector()
    sel.register(left, selectors.EVENT_READ, right)
    sel.register(right, selectors.EVENT_READ, left)
    sockets = (left, right)

    try:
        while True:
            events = sel.select()
            if not events:
                continue
            for key, _ in events:
                src = key.fileobj
                dst = key.data
                try:
                    data = src.recv(65536)
                except ConnectionResetError:
                    log("peer reset connection")
                    return
                if not data:
                    return
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
    try:
        log(f"accepted vsock client, connecting to {docker_sock}")
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
        return

    proxy_bidirectional(client, upstream)


def serve(vsock_port: int, docker_sock: str) -> None:
    listener = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((socket.VMADDR_CID_ANY, vsock_port))
    listener.listen()
    log(f"listening on vsock port {vsock_port}")

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
        description="Forward a guest vsock listener to Docker's Unix socket.",
    )
    parser.add_argument("--vsock-port", type=int, required=True)
    parser.add_argument("--docker-sock", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    serve(args.vsock_port, args.docker_sock)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
