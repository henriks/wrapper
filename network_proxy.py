"""CONNECT-only proxy with pinned public DNS and a guest socket relay.

Enforcement requires VM networking disabled; proxy variables alone are insufficient.
"""
import asyncio
from contextlib import contextmanager
import ipaddress
import json
import os
from pathlib import Path
import re
import socket
import threading

GUEST_SOCKET = '/run/sandbox-wrap/proxy.sock'
GUEST_PORT = 4750
PROXY_URL = f'http://127.0.0.1:{GUEST_PORT}'
# Bootstrap, Codex, source hosting, Python packages and Docker Hub downloads.
DEFAULT_ALLOW = (
    'api.openai.com', 'auth.openai.com', 'chatgpt.com',
    'mise.jdx.dev', 'mise-versions.jdx.dev', 'nodejs.org', 'registry.npmjs.org',
    'github.com', 'api.github.com', 'raw.githubusercontent.com',
    'objects.githubusercontent.com', 'release-assets.githubusercontent.com',
    'tuf-repo-cdn.sigstore.dev',
    'pypi.org', 'files.pythonhosted.org', 'awscli.amazonaws.com',
    'auth.docker.io', 'registry-1.docker.io', 'production.cloudfront.docker.com',
    'production.cloudflare.docker.com',
    'docker-images-prod.6aa30f8b08e16409b46e0173d6de2f56.r2.cloudflarestorage.com',
)


def hostname(value):
    try:
        return str(ipaddress.ip_address(value))
    except ValueError:
        pass
    value = value.rstrip('.').encode('idna').decode('ascii').lower()
    if not value or len(value) > 253 or any(
            not part or len(part) > 63 or not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]*[a-z0-9])?', part)
            for part in value.split('.')):
        raise ValueError('invalid hostname')
    return value


def domain_rule(value):
    return ('*.' + hostname(value[2:])) if value.startswith('*.') else hostname(value)


def policy(value):
    if not isinstance(value, dict) or set(value) - {'mode', 'allow', 'deny'}:
        raise ValueError('network accepts only mode, allow and deny')
    mode = value.get('mode', 'proxy')
    if mode not in ('direct', 'off', 'proxy'):
        raise ValueError('network mode must be direct, off or proxy')
    result = {'mode': mode, 'allow': list(DEFAULT_ALLOW) if mode == 'proxy' else None, 'deny': []}
    for key in ('allow', 'deny'):
        if key in value:
            if key == 'allow' and value[key] is None and mode == 'proxy':
                result[key] = None
                continue
            if mode != 'proxy' or not isinstance(value[key], list) or not all(isinstance(x, str) for x in value[key]):
                raise ValueError('allow and deny must be domain lists in proxy mode')
            result[key] = [domain_rule(x) for x in value[key]]
    return result


def configuration(project):
    path = Path(project) / '.sandbox' / 'config.json'
    if not path.exists():
        return None
    data = json.loads(path.read_text())
    if not isinstance(data, dict) or set(data) - {'network'}:
        raise ValueError('.sandbox/config.json accepts only a network object')
    return policy(data['network']) if 'network' in data else None


def select_policy(base, *, raw_net=False, unfiltered_net=False, no_net=False, allow=None, deny=None):
    if no_net or raw_net:
        return policy({'mode': 'off' if no_net else 'direct'})
    if unfiltered_net:
        return policy({'mode': 'proxy', 'allow': None})
    result = dict(base or policy({}))
    if (allow is not None or deny is not None) and result['mode'] != 'proxy':
        result = policy({})
    for key, values in (('allow', allow), ('deny', deny)):
        if values is not None:
            result[key] = [domain_rule(x) for x in values if x]
    return result


def permitted(host, rules):
    def matches(pattern):
        return host == pattern or (pattern.startswith('*.') and host.endswith(pattern[1:]))
    return (not any(matches(x) for x in rules['deny'])
            and (rules['allow'] is None or any(matches(x) for x in rules['allow'])))


async def public_connection(host, port):
    loop = asyncio.get_running_loop()
    answers = await loop.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    if not answers or any(not ipaddress.ip_address(a[4][0]).is_global
                          or ipaddress.ip_address(a[4][0]).is_multicast for a in answers):
        raise ValueError('destination is not public')
    # Connect the validated sockaddr directly: no second DNS lookup/rebinding.
    for family, kind, protocol, _, address in answers:
        connection = socket.socket(family, kind, protocol)
        connection.setblocking(False)
        try:
            await asyncio.wait_for(loop.sock_connect(connection, address), 3)
            return await asyncio.open_connection(sock=connection)
        except OSError:
            connection.close()
        except BaseException:
            connection.close()
            raise
    raise OSError('no validated destination address was reachable')


async def proxy(reader, writer, rules):
    upstream = None
    try:
        head = await asyncio.wait_for(reader.readuntil(b'\r\n\r\n'), 10)
        request = head.split(b'\r\n', 1)[0].decode('ascii')
        match = re.fullmatch(r'CONNECT (\[[0-9A-Fa-f:.]+\]|[^:\s/?#@%\[\]]+):443 HTTP/1\.[01]', request)
        if match is None:
            raise ValueError('CONNECT host:443 required')
        authority = match[1]
        host = str(ipaddress.IPv6Address(authority[1:-1])) if authority.startswith('[') else hostname(authority)
        if not permitted(host, rules):
            raise ValueError('destination is not allowed')
        remote, upstream = await asyncio.wait_for(public_connection(host, 443), 10)
        writer.write(b'HTTP/1.1 200 Connection Established\r\n\r\n')
        await writer.drain()
        await pump(reader, writer, remote, upstream)
    except (ValueError, OSError, asyncio.LimitOverrunError, asyncio.IncompleteReadError):
        if upstream is None:
            try:
                writer.write(b'HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n')
                await writer.drain()
            except OSError:
                pass
    finally:
        await close(writer, upstream)


async def close(*writers):
    streams = [writer for writer in writers if writer is not None]
    for writer in streams:
        writer.close()
    await asyncio.gather(*(writer.wait_closed() for writer in streams), return_exceptions=True)


def environment():
    return {key: PROXY_URL for key in ('http_proxy', 'https_proxy', 'HTTP_PROXY', 'HTTPS_PROXY')} | {
        'no_proxy': 'localhost,127.0.0.1,::1', 'NO_PROXY': 'localhost,127.0.0.1,::1'}


async def pump(reader, writer, remote, upstream):
    async def copy(source, destination):
        while data := await source.read(65536):
            destination.write(data)
            await destination.drain()
        if destination.can_write_eof():
            destination.write_eof()
    tasks = [asyncio.create_task(copy(reader, upstream)), asyncio.create_task(copy(remote, writer))]
    try:
        await asyncio.gather(*tasks)
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)


@contextmanager
def serve_proxy(path, rules):
    """Own a bounded asynchronous relay for the supervisor's lifetime."""
    rules = policy(rules)
    path = Path(path)
    path.unlink(missing_ok=True)
    # Bind synchronously so startup errors reach the lifecycle supervisor.
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        listener.bind(str(path))
        os.chmod(path, 0o600)
        listener.listen(64)
        listener.setblocking(False)
        loop = asyncio.new_event_loop()
        task = None
        async def run():
            connections = set()
            tasks = set()
            async def handle(connection):
                try:
                    reader, writer = await asyncio.open_connection(sock=connection)
                    await proxy(reader, writer, rules)
                finally:
                    connections.discard(connection)
                    connection.close()
            try:
                while True:
                    connection, _ = await loop.sock_accept(listener)
                    if len(tasks) >= 128:
                        connection.close()
                        continue
                    connections.add(connection)
                    pending = asyncio.create_task(handle(connection))
                    tasks.add(pending)
                    pending.add_done_callback(tasks.discard)
            finally:
                for pending in tasks:
                    pending.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
                for connection in connections:
                    connection.close()
        def worker():
            nonlocal task
            asyncio.set_event_loop(loop)
            task = loop.create_task(run())
            try:
                loop.run_until_complete(task)
            except asyncio.CancelledError:
                pass
            finally:
                loop.close()
        thread = threading.Thread(target=worker, daemon=True)
        thread.start()
        try:
            yield
        finally:
            def cancel():
                task.cancel()
            loop.call_soon_threadsafe(cancel)
            thread.join()
    finally:
        listener.close()
        path.unlink(missing_ok=True)


async def guest():
    async def handle(reader, writer):
        upstream = None
        try:
            remote, upstream = await asyncio.wait_for(asyncio.open_unix_connection(GUEST_SOCKET), 10)
            await pump(reader, writer, remote, upstream)
        except OSError:
            pass
        finally:
            await close(writer, upstream)
    # All guest interfaces so Docker bridge containers can reach the relay too.
    server = await asyncio.start_server(handle, '0.0.0.0', GUEST_PORT)
    async with server:
        await server.serve_forever()


if __name__ == '__main__':
    asyncio.run(guest())
