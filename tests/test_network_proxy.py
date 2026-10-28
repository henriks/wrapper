import asyncio
import json
from pathlib import Path
import random
import socket
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

import network_proxy as p


class PolicyTests(unittest.TestCase):
    def test_filtered_default_and_domain_rules(self):
        default = p.policy({'mode': 'proxy'})
        self.assertFalse(p.permitted('example.com', default))
        self.assertEqual(default['mode'], 'proxy')
        for name in ('api.openai.com', 'pypi.org', 'files.pythonhosted.org', 'registry.npmjs.org', 'registry-1.docker.io'):
            self.assertTrue(p.permitted(name, default))
        self.assertTrue(p.permitted('example.com', p.select_policy(None, unfiltered_net=True)))
        self.assertEqual(p.select_policy(None, raw_net=True)['mode'], 'direct')
        rules = p.policy({'mode': 'proxy', 'allow': ['Example.com', '*.example.org'], 'deny': ['bad.example.org']})
        for name in ('example.com', 'one.example.org', 'two.one.example.org'):
            self.assertTrue(p.permitted(name, rules))
        for name in ('notexample.com', 'example.org', 'bad.example.org', 'example.com.evil.org'):
            self.assertFalse(p.permitted(name, rules))
        self.assertFalse(p.permitted('example.com', p.policy({'mode': 'proxy', 'allow': []})))
        self.assertEqual(p.policy(default), default)

    def test_cli_lists_replace_configured_lists_independently(self):
        base = p.policy({'mode': 'proxy', 'allow': ['old.org'], 'deny': ['bad.org']})
        self.assertEqual(p.select_policy(base, allow=['new.org'])['allow'], ['new.org'])
        self.assertEqual(p.select_policy(base, allow=['new.org'])['deny'], ['bad.org'])
        self.assertEqual(p.select_policy(base, deny=[''])['deny'], [])
        self.assertEqual(p.select_policy(base, allow=[''])['allow'], [])
        self.assertEqual(p.select_policy(None, deny=['bad.org'])['mode'], 'proxy')
        self.assertEqual(p.select_policy(base, no_net=True)['mode'], 'off')
        self.assertEqual(base['allow'], ['old.org'])

    def test_config_validation(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertIsNone(p.configuration(root))
            directory = Path(root) / '.sandbox'
            directory.mkdir()
            path = directory / 'config.json'
            for value in ([], {'unknown': 3}, {'network': []}, {'network': {'mode': 'wat'}},
                          {'network': {'mode': 'proxy', 'allow': '*'}},
                          {'network': {'mode': 'proxy', 'deny': [2]}},
                          {'network': {'mode': 'proxy', 'allow': ['*']}}):
                path.write_text(json.dumps(value))
                with self.assertRaises(ValueError):
                    p.configuration(root)
            path.write_text('{"network":{"mode":"proxy","deny":["*.example.com"]}}')
            self.assertEqual(p.configuration(root)['deny'], ['*.example.com'])


class ProxyTests(unittest.IsolatedAsyncioTestCase):
    async def exchange(self, data, rules=None, connect=None):
        output = bytearray()
        class Writer:
            def write(self, value): output.extend(value)
            async def drain(self): pass
            def close(self): pass
            async def wait_closed(self): pass
            def can_write_eof(self): return False
        reader = asyncio.StreamReader(limit=65536)
        reader.feed_data(data)
        reader.feed_eof()
        with patch.object(p, 'public_connection', connect or AsyncMock(side_effect=ValueError('blocked'))) as dial:
            await p.proxy(reader, Writer(), rules or p.policy({'mode': 'proxy', 'allow': ['example.com']}))
        return bytes(output), dial

    async def test_private_and_mixed_dns_answers_rejected_before_connect(self):
        loop = asyncio.get_running_loop()
        for addresses in (['127.0.0.1'], ['10.0.0.1'], ['169.254.169.254'], ['::1'], ['fc00::1'],
                          ['224.0.0.1'], ['8.8.8.8', '192.168.0.1']):
            answers = [(socket.AF_INET6 if ':' in x else socket.AF_INET, socket.SOCK_STREAM, 6, '', (x, 443)) for x in addresses]
            with patch.object(loop, 'getaddrinfo', AsyncMock(return_value=answers)), \
                 patch.object(loop, 'sock_connect', AsyncMock()) as dial:
                with self.assertRaises(ValueError):
                    await p.public_connection('example.com', 443)
                dial.assert_not_called()

    async def test_connect_uses_validated_ip_without_second_dns_lookup(self):
        loop = asyncio.get_running_loop()
        address = ('93.184.216.34', 443)
        with patch.object(loop, 'getaddrinfo', AsyncMock(return_value=[(socket.AF_INET, socket.SOCK_STREAM, 6, '', address)])) as dns, \
             patch.object(loop, 'sock_connect', AsyncMock()) as dial, \
             patch.object(p.asyncio, 'open_connection', AsyncMock(return_value=('reader', 'writer'))) as opened:
            self.assertEqual(await p.public_connection('example.com', 443), ('reader', 'writer'))
            dns.assert_awaited_once()
            self.assertEqual(dial.call_args.args[1], address)
            self.assertEqual(set(opened.call_args.kwargs), {'sock'})
            opened.call_args.kwargs['sock'].close()

    async def test_deny_and_malformed_requests_never_connect(self):
        for data in (b'CONNECT example.com:22 HTTP/1.1\r\n\r\n',
                     b'CONNECT user:pass@example.com:443 HTTP/1.1\r\n\r\n',
                     b'GET https://example.com/ HTTP/1.1\r\n\r\n',
                     b'GET http://example.com:0/ HTTP/1.1\r\n\r\n',
                     b'CONNECT example.com:443 HTTP/1.1\r\nBad Header: hi\r\n\r\n',
                     b'CONNECT example.com:443 HTTP/1.1\r\n\r\n'):
            output, dial = await self.exchange(data, p.policy({'mode': 'proxy', 'deny': ['example.com']}))
            self.assertIn(b'403 Forbidden', output)
            dial.assert_not_called()

    async def test_connect_preserves_buffered_binary_payload(self):
        payload = bytes(range(256)) * 1024
        received = bytearray()
        async def origin(reader, writer):
            received.extend(await reader.read())
            writer.write(payload[::-1])
            await writer.drain()
            await p.close(writer)
        server = await asyncio.start_server(origin, '127.0.0.1', 0)
        port = server.sockets[0].getsockname()[1]
        async with server:
            async def connect(*args):
                return await asyncio.open_connection('127.0.0.1', port)
            output, _ = await self.exchange(b'CONNECT example.com:443 HTTP/1.1\r\nIgnored: header\r\n\r\n' + payload,
                                            connect=connect)
        self.assertEqual(output, b'HTTP/1.1 200 Connection Established\r\n\r\n' + payload[::-1])
        self.assertEqual(received, payload)

    async def test_non_connect_methods_and_invalid_authorities_never_connect(self):
        requests = [f'{method} http://example.com/ HTTP/1.1' for method in ('GET', 'POST', 'HEAD', 'PUT', 'DELETE', 'OPTIONS')]
        requests += [f'CONNECT {authority} HTTP/1.1' for authority in (
            'example.com', 'example.com:80', 'example.com:22', 'user@example.com:443',
            'example.com:443/path', 'example.com:443?query', 'example.com:443#fragment',
            '//example.com:443', 'example.com:443:443', '[not-ipv6]:443')]
        for request in requests:
            output, dial = await self.exchange(request.encode() + b'\r\n\r\n')
            self.assertIn(b'403 Forbidden', output)
            dial.assert_not_called()

    async def test_tunnel_preserves_both_half_close_directions(self):
        for upstream_first in (False, True):
            received = asyncio.get_running_loop().create_future()
            async def origin(reader, writer):
                try:
                    if upstream_first:
                        writer.write(b'server half-close')
                        await writer.drain()
                        writer.write_eof()
                    data = await reader.read()
                    if not upstream_first:
                        writer.write(b'server half-close')
                        await writer.drain()
                        writer.write_eof()
                    received.set_result(data)
                finally:
                    await p.close(writer)
            server = await asyncio.start_server(origin, '127.0.0.1', 0)
            async def bridge(reader, writer):
                remote, upstream = await asyncio.open_connection('127.0.0.1', server.sockets[0].getsockname()[1])
                try:
                    await p.pump(reader, writer, remote, upstream)
                finally:
                    await p.close(writer, upstream)
            relay = await asyncio.start_server(bridge, '127.0.0.1', 0)
            async with server, relay:
                reader, writer = await asyncio.open_connection('127.0.0.1', relay.sockets[0].getsockname()[1])
                try:
                    if upstream_first:
                        self.assertEqual(await asyncio.wait_for(reader.read(), 2), b'server half-close')
                    writer.write(b'client half-close')
                    await writer.drain()
                    writer.write_eof()
                    if not upstream_first:
                        self.assertEqual(await asyncio.wait_for(reader.read(), 2), b'server half-close')
                    self.assertEqual(await asyncio.wait_for(received, 2), b'client half-close')
                finally:
                    await p.close(writer)

    async def test_seeded_malformed_input(self):
        rng = random.Random(405)
        for _ in range(500):
            data = rng.randbytes(rng.randrange(300)) + b'\r\n\r\n'
            output, dial = await self.exchange(data)
            self.assertIn(b'403 Forbidden', output)
            dial.assert_not_called()
        output, dial = await self.exchange(b'A' * 70000 + b'\r\n\r\n')
        self.assertIn(b'403 Forbidden', output)
        dial.assert_not_called()


class LifecycleTests(unittest.TestCase):
    def test_socket_cleanup_with_active_connection_and_restart(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'proxy.sock'
            for _ in range(2):
                with p.serve_proxy(path, {'mode': 'proxy'}):
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(3)
                        client.connect(str(path))
                        client.sendall(b'CONNECT localhost:443 HTTP/1.1\r\n\r\n')
                        self.assertIn(b'403 Forbidden', client.recv(4096))
                    idle = socket.socket(socket.AF_UNIX)
                    idle.connect(str(path))
                idle.close()
                self.assertFalse(path.exists())
