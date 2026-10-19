#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
python3 -m unittest discover -s tests -v
python3 -m py_compile sandbox-wrap host_rpc.py vm_session.py guest/hostctl
bash -n guest/daemon guest/prepare guest/run
case "${1:-required}" in
    offline) ;;
    required|live)
        timeout 300 ./codex-wrap --rebuild-image
        python3 tests/live.py
        python3 tests/live_shared.py
        ;;
    *) echo 'usage: ./validate.sh [offline|required|live]' >&2; exit 2 ;;
esac
