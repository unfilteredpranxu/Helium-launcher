"""Handcrafted Mach-O metadata, NOT a linked app or device runtime test."""
import importlib.util
from pathlib import Path
import struct
import unittest

spec = importlib.util.spec_from_file_location("verify_attached", Path(__file__).parents[1] / "tools/ci/verify-attached-ios-game.py")
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


def fixture(cpu=0x100000C, kind=2, platform=2, version=15 << 16):
    return struct.pack("<8I6I", 0xFEEDFACF, cpu, 0, kind, 1, 24, 0, 0, 0x32, 24, platform, version, 0, 0)


class BinaryContract(unittest.TestCase):
    def test_handcrafted_device_deployment(self):
        self.assertEqual(verify.ios_version(fixture()), 15 << 16)

    def test_wrong_architecture_or_library(self):
        for data in (fixture(cpu=7), fixture(kind=6), fixture(platform=7), b"not a game"):
            with self.assertRaises(ValueError):
                verify.ios_version(data)

    def test_malformed_load_commands_reject(self):
        for data in (fixture()[:-1], fixture()[:32], fixture()[:36] + struct.pack("<I", 0xFFFFFFFF) + fixture()[40:]):
            with self.assertRaises(ValueError):
                verify.ios_version(data)

    def test_legacy_real_ios_command(self):
        data = struct.pack("<8I4I", 0xFEEDFACF, 0x100000C, 0, 2, 1, 16, 0, 0, 0x25, 16, 15 << 16, 0)
        self.assertEqual(verify.ios_version(data), 15 << 16)
