import io
import json
import random
import socket
import tempfile
import threading
import time
from types import SimpleNamespace
from pathlib import Path
import unittest
from unittest.mock import patch

import host_rpc as rpc


def buffered_connection(payload):
    return SimpleNamespace(recv=io.BytesIO(payload).read, settimeout=lambda _: None)


class RpcTests(unittest.TestCase):
    def test_status_and_protocol_errors(self):
        status = {'state': 'running', 'project': '/project', 'wrapper_pid': 42}
        request = {'jsonrpc': '2.0', 'id': 'test', 'method': 'status'}
        reply = rpc.response(json.dumps(request), status)
        self.assertEqual(reply, {'jsonrpc': '2.0', 'id': 'test', 'result': status})
        for payload, code in ((b'bad json', -32700), (b'[]', -32600),
                              (b'{"jsonrpc":"2.0","id":true,"method":"status"}', -32600),
                              (b'{"jsonrpc":"2.0","id":1,"method":"exec"}', -32601),
                              (b'{"jsonrpc":"2.0","id":1,"method":"status","params":[1]}', -32602)):
            self.assertEqual(rpc.response(payload, status)['error']['code'], code)
        self.assertIsNone(rpc.response(b'{"jsonrpc":"2.0","method":"status"}', status))

    def test_seeded_protocol_and_framing_fuzz(self):
        rng = random.Random(218)
        seeds = [b'{"jsonrpc":"2.0","id":1,"method":"status"}', b'[]', b'null',
                 b'{"jsonrpc":"2.0","id":1,"result":{"state":"running"}}',
                 b'{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown"}}',
                 b'{"jsonrpc":"2.0","id":NaN,"method":"status"}', b'\xff', b'[' * 2000 + b']' * 2000]
        for _ in range(5000):
            payload = bytearray(rng.choice(seeds)) if rng.randrange(2) else bytearray(rng.randbytes(rng.randrange(256)))
            for _ in range(rng.randrange(5)):
                if payload:
                    payload[rng.randrange(len(payload))] = rng.randrange(256)
            reply = rpc.response(bytes(payload), {'state': 'running'})
            if reply is not None:
                encoded = json.dumps(reply, allow_nan=False)
                self.assertEqual(json.loads(encoded)['jsonrpc'], '2.0')
                self.assertTrue('result' in reply or 'error' in reply)
            try:
                result = rpc.status_result(bytes(payload))
            except (ValueError, RecursionError):
                pass
            else:
                self.assertIsInstance(result, dict)
            framed = bytes(payload) + (b'\n' if rng.randrange(2) else b'')
            try:
                result = rpc.read_frame(buffered_connection(framed))
            except ValueError:
                continue
            self.assertLessEqual(len(result), rpc.MAX_MESSAGE)
            self.assertTrue(result.endswith(b'\n'))
        for payload in (b'', b'{}', b'x' * rpc.MAX_MESSAGE + b'\n'):
            with self.assertRaises(ValueError):
                rpc.read_frame(buffered_connection(payload))

    def test_socket_lifecycle_and_reconnect(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'host.sock'
            status = {'state': 'running', 'project': directory, 'wrapper_pid': 41}
            for pid in (41, 42):
                status['wrapper_pid'] = pid
                with rpc.serve_status(path, status):
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                    self.assertEqual(rpc.get_status(path), status)
                    status['state'] = 'stopping'
                    self.assertEqual(rpc.get_status(path)['state'], 'stopping')
                    # A malformed or oversized client does not kill the server.
                    for payload in (b'bad\n', b'x' * (rpc.MAX_MESSAGE + 1)):
                        with socket.socket(socket.AF_UNIX) as client:
                            client.settimeout(2)
                            client.connect(str(path))
                            client.sendall(payload)
                            self.assertIn('error', json.loads(client.recv(4096)))
                    self.assertEqual(rpc.get_status(path)['wrapper_pid'], pid)
                self.assertFalse(path.exists())

    def test_slow_client_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'host.sock'
            with patch.object(rpc, 'TIMEOUT', 0.1), rpc.serve_status(path, {'state': 'running'}):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(2)
                    client.connect(str(path))
                    client.sendall(b'{')
                    self.assertEqual(client.recv(1), b'')
                self.assertEqual(rpc.get_status(path)['state'], 'running')

    def test_trickling_client_cannot_extend_request_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'host.sock'
            with patch.object(rpc, 'TIMEOUT', 0.2), rpc.serve_status(path, {'state': 'running'}):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(2)
                    client.connect(str(path))
                    finished = threading.Event()
                    def trickle():
                        while not finished.wait(0.03):
                            try:
                                client.sendall(b' ')
                            except OSError:
                                return
                    worker = threading.Thread(target=trickle, daemon=True)
                    worker.start()
                    started = time.monotonic()
                    try:
                        self.assertEqual(client.recv(1), b'')
                        self.assertLess(time.monotonic() - started, 1)
                    finally:
                        finished.set()
                        worker.join(timeout=1)
                self.assertEqual(rpc.get_status(path)['state'], 'running')


if __name__ == '__main__':
    unittest.main()
