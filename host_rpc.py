"""Status-only JSON-RPC, one bounded newline-delimited message per connection."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
import socketserver
import threading
import time

GUEST_SOCKET = '/run/sandbox-wrap/host.sock'
MAX_MESSAGE = 65536
TIMEOUT = 5


def error(code, message, request_id=None):
    return {'jsonrpc': '2.0', 'id': request_id, 'error': {'code': code, 'message': message}}


def decode(payload):
    def invalid_constant(value):
        raise ValueError('non-JSON numeric constant')
    if isinstance(payload, bytes):
        payload = payload.decode('utf-8')
    return json.loads(payload, parse_constant=invalid_constant)


def response(payload, status):
    try:
        request = decode(payload)
    except (ValueError, UnicodeError, RecursionError):
        return error(-32700, 'Parse error')
    if not isinstance(request, dict):
        return error(-32600, 'Invalid request')
    request_id = request.get('id')
    if (request.get('jsonrpc') != '2.0' or not isinstance(request.get('method'), str)
            or type(request_id) not in (str, int, type(None))):
        return error(-32600, 'Invalid request')
    if 'id' not in request:
        return None
    if request['method'] != 'status':
        return error(-32601, 'Method not found', request_id)
    if request.get('params', {}) not in ({}, []):
        return error(-32602, 'status takes no parameters', request_id)
    return {'jsonrpc': '2.0', 'id': request_id, 'result': dict(status)}


def read_frame(connection):
    deadline = time.monotonic() + TIMEOUT
    payload = bytearray()
    while len(payload) <= MAX_MESSAGE:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('RPC request timed out')
        connection.settimeout(remaining)
        chunk = connection.recv(min(4096, MAX_MESSAGE + 1 - len(payload)))
        if not chunk:
            break
        payload.extend(chunk)
        newline = payload.find(b'\n')
        if newline >= 0 and newline + 1 <= MAX_MESSAGE:
            return bytes(payload[:newline + 1])
    raise ValueError('RPC message must end with a newline and fit in 64 KiB')


@contextmanager
def serve_status(path, status):
    """Serve sequential requests with bounded reads; never spawn per-client threads."""
    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            self.request.settimeout(TIMEOUT)
            try:
                try:
                    reply = response(read_frame(self.request), status)
                except ValueError as exc:
                    reply = error(-32600, str(exc))
                if reply is not None:
                    encoded = json.dumps(reply, allow_nan=False).encode() + b'\n'
                    if len(encoded) > MAX_MESSAGE:
                        encoded = json.dumps(error(-32600, 'Response exceeds 64 KiB')).encode() + b'\n'
                    self.request.settimeout(TIMEOUT)
                    self.request.sendall(encoded)
            except (OSError, ValueError):
                pass  # Disconnected or timed-out clients do not affect the wrapper.

    path = Path(path)
    path.unlink(missing_ok=True)
    server = socketserver.UnixStreamServer(str(path), Handler)
    os.chmod(path, 0o600)
    thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.1}, daemon=True)
    thread.start()
    try:
        yield
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
        path.unlink(missing_ok=True)


def status_result(payload):
    reply = decode(payload)
    if (not isinstance(reply, dict) or reply.get('jsonrpc') != '2.0'
            or type(reply.get('id')) is not int or reply['id'] != 1):
        raise ValueError('Invalid RPC response')
    if 'error' in reply:
        raise ValueError(f"Host RPC failed: {reply['error']}")
    if not isinstance(reply.get('result'), dict):
        raise ValueError('Invalid status result')
    return reply['result']


def get_status(path=GUEST_SOCKET):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(TIMEOUT)
        connection.connect(str(path))
        connection.sendall(b'{"jsonrpc":"2.0","id":1,"method":"status"}\n')
        return status_result(read_frame(connection))
