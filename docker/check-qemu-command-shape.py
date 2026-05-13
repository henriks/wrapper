#!/usr/bin/env python3
"""Check Docker VM QEMU command construction for q35 and microvm."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def load_sandbox_wrap():
    loader = importlib.machinery.SourceFileLoader(
        "sandbox_wrap_under_test",
        str(ROOT / "sandbox-wrap"),
    )
    spec = importlib.util.spec_from_loader(loader.name, loader)
    if spec is None:
        raise RuntimeError("failed to build import spec for sandbox-wrap")
    module = importlib.util.module_from_spec(spec)
    sys.modules[loader.name] = module
    loader.exec_module(module)
    return module


def make_manager(sw, machine_type: str):
    manager = sw.DockerVmManager(
        "/project",
        "/project/.sandbox",
        str(ROOT),
        "codex",
        network_enabled=True,
        machine_type=machine_type,
    )
    manager.host_tools = {"qemu-system-x86_64": "qemu-system-x86_64"}
    manager.network = sw.DockerVmNetwork(
        guest_ip="10.0.2.15",
        prefix_len=24,
        gateway_ip="10.0.2.2",
        guest_mac="02:fc:12:34:56:78",
        dns_ip="10.0.2.3",
    )
    manager.host_docker_port = 10075
    manager.host_payload_port = 10076
    manager.load_manifest = lambda: {
        "artifacts": {
            "kernel": "/artifact/vmlinuz",
            "initrd": "/artifact/initrd.img",
            "rootfs": "/artifact/rootfs.raw",
        },
        "vm": {
            "kernel_cmdline": "root=/dev/vda",
            "memory_bytes": 2 * 1024 * 1024 * 1024,
            "cpus": 2,
            "virtiofs_tag": "workspace",
        },
        "guest": {
            "docker_tcp_port": 1075,
            "payload_tcp_port": 1076,
        },
    }
    return manager


def assert_contains(command: list[str], expected: str) -> None:
    if expected not in command and expected not in " ".join(command):
        raise AssertionError(f"missing expected command fragment: {expected}")


def check_q35(sw) -> None:
    command = make_manager(
        sw,
        sw.DOCKER_VM_MACHINE_Q35,
    ).build_qemu_command()
    assert_contains(command, "-machine q35,accel=kvm,memory-backend=mem")
    assert_contains(command, "virtio-blk-pci,drive=rootfs")
    assert_contains(command, "virtio-rng-pci,rng=rng0")
    assert_contains(command, "vhost-user-fs-pci,chardev=charfs,tag=workspace")
    assert "-nic" in command
    assert "-netdev" not in command


def check_microvm(sw) -> None:
    command = make_manager(
        sw,
        sw.DOCKER_VM_MACHINE_MICROVM,
    ).build_qemu_command()
    assert_contains(
        command,
        "-machine microvm,acpi=off,memory-backend=mem,isa-serial=on",
    )
    assert "-cpu" in command
    assert_contains(command, "virtio-blk-device,drive=rootfs")
    assert_contains(command, "virtio-rng-device,rng=rng0")
    assert_contains(command, "vhost-user-fs-device,chardev=charfs,tag=workspace")
    assert "-netdev" in command
    assert "-nic" not in command
    assert_contains(
        command,
        "virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78",
    )


def main() -> None:
    sw = load_sandbox_wrap()
    check_q35(sw)
    check_microvm(sw)
    print("qemu-command-shape-ok")


if __name__ == "__main__":
    main()
