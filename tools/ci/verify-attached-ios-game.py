#!/usr/bin/env python3
"""Verify the actual linked game before making the isolated, asset-free test IPA.
MIT; Copyright (c) 2026 Horizon contributors.
"""
import hashlib
import mmap
from pathlib import Path
import plistlib
import struct
import subprocess
import sys


def ios_version(header: bytes) -> int:
    if len(header) < 32:
        raise ValueError("truncated Mach-O")
    magic, cpu, _, kind, count, size, _, _ = struct.unpack_from("<8I", header)
    if magic != 0xFEEDFACF or cpu != 0x0100000C or kind != 2:
        raise ValueError("expected a thin arm64 Mach-O EXECUTABLE, not GameShell/library/fat image")
    if size > 1024 * 1024 or len(header) < 32 + size or count > size // 8:
        raise ValueError("invalid load command bounds")
    offset = 32
    version = None
    for _ in range(count):
        if offset + 8 > 32 + size:
            raise ValueError("truncated load command")
        cmd, length = struct.unpack_from("<2I", header, offset)
        if length < 8 or offset + length > 32 + size:
            raise ValueError("invalid load command size")
        if cmd == 0x32:  # LC_BUILD_VERSION
            if length < 24:
                raise ValueError("truncated build version")
            platform, version = struct.unpack_from("<2I", header, offset + 8)
            if platform != 2:
                raise ValueError("not device iOS (simulator/macOS is insufficient)")
        elif cmd == 0x25:  # LC_VERSION_MIN_IPHONEOS
            if length < 16:
                raise ValueError("truncated minimum OS")
            version = struct.unpack_from("<I", header, offset + 8)[0]
        offset += length
    if offset != 32 + size or version is None:
        raise ValueError("missing/invalid iOS deployment commands")
    return version


def verify(app: Path, rust: Path) -> None:
    binary = app / "Horizon"
    with binary.open("rb") as file:
        head = file.read(1024 * 1024 + 32)
    if ios_version(head) != 15 << 16:
        raise ValueError("expected the preserved iOS 15.0 minimum")
    for file in (binary, rust):
        if not 0 < file.stat().st_size <= 256 * 1024 * 1024:
            raise ValueError("unexpected game executable size")
    def digest(file):
        with file.open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()
    if digest(binary) != digest(rust):
        raise ValueError("bundle executable differs from the opt-in Rust game")
    with binary.open("rb") as file, mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ) as data:
        if data.find(b"attached-one-section") < 0:
            raise ValueError("native test mode absent from linked executable")
    framework = app / "Frameworks/HorizonLauncherKit.framework/HorizonLauncherKit"
    result = subprocess.check_output(["xcrun", "nm", "-g", str(framework)], text=True)
    defined = {parts[-1] for line in result.splitlines() if len(parts := line.split()) >= 3 and parts[-2] == "T"}
    for name in ("abi", "mode", "request", "publish", "cancel"):
        if "_horizon_native_display_" + name not in defined:
            raise ValueError("missing native host definition: " + name)
    for path in app.rglob("*"):
        if path.name in {"horizon-resources.json", "level.dat", "helium-metal-test.txt"} or path.suffix == ".jar":
            raise ValueError("asset/save/config must NOT be bundled: " + path.name)
    info = app / "Info.plist"
    with info.open("rb") as stream:
        value = plistlib.load(stream)
    if value["CFBundleExecutable"] != "Horizon" or value["MinimumOSVersion"] != "15.0":
        raise ValueError("wrong actual game bundle contract")
    value.update(CFBundleIdentifier="dev.helium.g06ctest", CFBundleDisplayName="Helium G06C Test", HeliumNativeMetalTest=True)
    with info.open("wb") as stream:
        plistlib.dump(value, stream)
    print("Verified actual opt-in Rust executable == bundle executable; thin arm64 device iOS15.0")
    print("Executable bytes:", binary.stat().st_size, "SHA256:", digest(binary))
    print("Five ABI v1 host exports defined; proprietary assets/saves/config absent; isolated bundle ID applied")
    print("Configuration file absent: default OFF. Device/world runtime remains UNVERIFIED.")


if __name__ == "__main__":
    verify(Path(sys.argv[1]), Path(sys.argv[2]))
