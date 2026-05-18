#!/usr/bin/env python3
"""Offline tests for guest init/build shell syntax."""

from __future__ import annotations

import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class GuestShellTests(unittest.TestCase):
    def assert_valid_shell_syntax(self, relative: str) -> None:
        result = subprocess.run(
            ["sh", "-n", str(ROOT / relative)],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_guest_init_has_valid_shell_syntax(self) -> None:
        self.assert_valid_shell_syntax("docker/guest-init.sh")

    def test_build_appliance_has_valid_shell_syntax(self) -> None:
        self.assert_valid_shell_syntax("docker/build-appliance.sh")

    def test_guest_init_does_not_preserve_workspace_alias(self) -> None:
        text = (ROOT / "docker/guest-init.sh").read_text(encoding="utf-8")
        self.assertNotIn("falling back to /workspace", text)
        self.assertNotIn("mount --bind \"${PROJECT_PATH}\" /workspace", text)
        self.assertIn("unsupported guest project path", text)

    def test_guest_init_does_not_write_hidden_sandbox_logs_by_default(self) -> None:
        text = (ROOT / "docker/guest-init.sh").read_text(encoding="utf-8")
        self.assertNotIn("${PROJECT_PATH}/.sandbox", text)
        self.assertIn("GUEST_LOG_DIR", text)
        self.assertIn("/run/guest-http-smoke.log", text)
        self.assertIn("mirroring guest service logs", text)

    def test_guest_init_disables_ipv6_before_guest_iface_up(self) -> None:
        text = (ROOT / "docker/guest-init.sh").read_text(encoding="utf-8")
        self.assertIn("disable_ipv6_for()", text)
        self.assertIn("/proc/sys/net/ipv6/conf/${target}/disable_ipv6", text)
        self.assertIn("disable_guest_ipv6_defaults", text)
        defaults_index = text.index("disable_guest_ipv6_defaults\nip link set lo up")
        iface_disable_index = text.index("disable_ipv6_for \"${IFACE}\"")
        iface_up_index = text.index("ip link set \"${IFACE}\" up")
        self.assertLess(defaults_index, iface_disable_index)
        self.assertLess(iface_disable_index, iface_up_index)


if __name__ == "__main__":
    unittest.main()
