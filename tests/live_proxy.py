"""Real VM proxy use, host policy and direct-egress denial including guest root."""
import json
from pathlib import Path
import subprocess
import time
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    with tempfile.TemporaryDirectory(prefix='wrapper-proxy-') as directory:
        project = Path(directory) / 'project'
        project.mkdir()
        state = project / '.sandbox'
        state.mkdir()
        config = state / 'config.json'
        text = json.dumps({'network': {'deny': ['example.org']}})
        config.write_text(text)
        codex = Path(directory) / 'codex'
        codex.mkdir()
        command = [str(ROOT / 'codex-wrap'), '--project', str(project), '--codex-home', str(codex)]
        try:
            subprocess.run([*command, '--', 'bash', '-ec', '''
                test "$HTTPS_PROXY" = http://127.0.0.1:4750
                curl -fsS --max-time 30 https://pypi.org/simple/pip/ >/dev/null
                test "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 http://example.com)" = 403
                if curl -fsS --max-time 10 https://example.com >/dev/null; then exit 1; fi
                if curl --noproxy '' -fsS --max-time 10 https://169.254.169.254 >/dev/null; then exit 1; fi
                python3 - <<'CHECK'
import json, socket, urllib.request
with urllib.request.urlopen('https://pypi.org/pypi/idna/json', timeout=15) as response:
    package = json.load(response)
with urllib.request.urlopen(package['urls'][0]['url'], timeout=15) as response:
    assert response.read(64), 'PyPI artifact download failed'
for target in [('1.1.1.1', 443), ('2606:4700:4700::1111', 443)]:
    try:
        connection = socket.create_connection(target, timeout=3)
    except OSError:
        continue
    connection.close()
    raise AssertionError(f'direct egress succeeded: {target}')
CHECK
                docker pull alpine:3.20
                gateway=$(docker network inspect bridge --format '{{(index .IPAM.Config 0).Gateway}}')
                docker pull curlimages/curl:latest
                docker run --rm curlimages/curl:latest --proxy "http://$gateway:4750" -fsS --max-time 30 https://pypi.org/simple/pip/ >/dev/null
                if docker run --rm --privileged --network host alpine:3.20 wget -q -T 3 -Y off -O /dev/null http://1.1.1.1; then
                    echo 'guest root bypassed proxy' >&2; exit 1
                fi
                hostctl status > status.json
            '''], check=True, timeout=240)
            status = json.loads((project / 'status.json').read_text())
            assert status['proxy'] is True and status['network'] is False
            assert not (state / 'proxy.sock').exists()
            assert config.read_text() == text
            assert json.loads((state / 'launch.json').read_text())['proxy']['allow'] is not None
            subprocess.run([*command, '--unfiltered-net', '--', 'bash', '-ec', '''
                curl -fsS --max-time 30 https://example.org >/dev/null
                if curl --noproxy '' -fsS --max-time 10 https://169.254.169.254 >/dev/null; then exit 1; fi
            '''], check=True, timeout=120)
            effective = json.loads((state / 'launch.json').read_text())
            assert effective['proxy']['allow'] is None and effective['proxy']['deny'] == []
            assert not effective['network']
            assert config.read_text() == text
            # Override deny alone while retaining the built-in allowlist.
            subprocess.run([*command, '--deny-domain', 'pypi.org', '--', 'bash', '-ec', '''
                curl -fsS --max-time 30 https://nodejs.org/ >/dev/null
                if curl -fsS --max-time 10 https://pypi.org/ >/dev/null; then exit 1; fi
            '''], check=True, timeout=120)
            assert config.read_text() == text
            # CLI overrides are inherited by concurrent sessions even when the
            # saved config contains a different list.
            ready = project / 'ready'
            first = subprocess.Popen([*command, '--deny-domain', 'pypi.org', '--', 'bash', '-ec',
                                      'touch ready; while :; do sleep 1; done'])
            try:
                deadline = time.monotonic() + 120
                while not ready.exists():
                    assert first.poll() is None, 'proxy owner exited before admission'
                    if time.monotonic() > deadline:
                        raise TimeoutError('proxy owner did not become ready')
                    time.sleep(0.05)
                subprocess.run([*command, '--', 'bash', '-ec',
                                'curl -fsS --max-time 30 https://nodejs.org/ >/dev/null; if curl -fsS --max-time 10 https://pypi.org/ >/dev/null; then exit 1; fi; hostctl status > joined.json'],
                               check=True, timeout=60)
                # Configuration edits belong to the next lifecycle, even if invalid.
                config.write_text('{')
                subprocess.run([*command, '--entrypoint', 'bash', '--', '-ec',
                                'if curl -fsS --max-time 10 https://pypi.org/ >/dev/null; then exit 1; fi; hostctl status > invalid-config-joined.json'],
                               check=True, timeout=60)
                assert json.loads((project / 'invalid-config-joined.json').read_text())['wrapper_pid'] == json.loads((project / 'joined.json').read_text())['wrapper_pid']
                assert config.read_text() == '{'
                rejected = subprocess.run([*command, '--deny-domain', 'changed.org', '--', 'true'], timeout=30)
                assert rejected.returncode != 0
                assert first.poll() is None
            finally:
                if first.poll() is None:
                    first.terminate()
                first.wait(timeout=60)
            assert not (state / 'proxy.sock').exists()
            rejected = subprocess.run([*command, '--', 'true'], capture_output=True, timeout=30)
            assert rejected.returncode != 0, 'new lifecycle accepted malformed configuration'
            assert config.read_text() == '{'
            config.write_text(text)
            # Retaining the socket bridge must not leave a usable proxy in off mode.
            subprocess.run([*command, '--no-net', '--', 'bash', '-ec',
                            'test -z "${HTTPS_PROXY:-}"; if curl --proxy http://127.0.0.1:4750 --max-time 3 https://example.com; then exit 1; fi'],
                           check=True, timeout=120)
            subprocess.run([*command, '--raw-net', '--', 'bash', '-ec',
                            'test -z "${HTTPS_PROXY:-}"; curl -fsS --max-time 30 https://example.com >/dev/null'],
                           check=True, timeout=120)
            assert config.read_text() == text
            subprocess.run([*command, '--', 'bash', '-ec',
                            'test "$HTTPS_PROXY" = http://127.0.0.1:4750; if curl -fsS --max-time 10 https://example.com >/dev/null; then exit 1; fi'],
                           check=True, timeout=120)
            config.write_text('{"network":{"mode":"direct"}}')
            subprocess.run([*command, '--', 'bash', '-ec',
                            'test -z "${HTTPS_PROXY:-}"; curl -fsS --max-time 30 https://example.com >/dev/null'],
                           check=True, timeout=120)
            config.write_text('{"network":{"mode":"proxy","allow":null}}')
            subprocess.run([*command, '--', 'bash', '-ec',
                            'test "$HTTPS_PROXY" = http://127.0.0.1:4750; curl -fsS --max-time 30 https://example.org >/dev/null'],
                           check=True, timeout=120)
            config.write_text(text)
            subprocess.run([*command, '--reset'], check=True, timeout=60)
            assert config.read_text() == text
            print('Proxy routing, ACL override, private-IP rejection, Docker and direct-egress denial passed', flush=True)
        finally:
            if (state / 'supervisor.error').exists():
                print((state / 'supervisor.log').read_text(), flush=True)
            subprocess.run([*command, '--reset'], check=True, timeout=60)


if __name__ == '__main__':
    main()
