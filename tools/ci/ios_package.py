"""Build and verify the exact unsigned S1H archive. Python 3.11+."""
from dataclasses import dataclass
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import plistlib
import re
import stat
import struct
import subprocess
import tempfile
import zipfile


PROBE = ("HeliumMetalProbe", "HeliumMetalProbe", "HeliumProbeKit", "dev.helium.metalprobe")
GAME = ("HorizonGame", "Horizon", "HorizonLauncherKit", "dev.helium.g06ctest")
MAX_BYTES = 256 * 1024 * 1024
ORIENTATIONS = {
    "UIInterfaceOrientationPortrait", "UIInterfaceOrientationPortraitUpsideDown",
    "UIInterfaceOrientationLandscapeLeft", "UIInterfaceOrientationLandscapeRight",
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


@dataclass
class Image:
    minimum: int
    id: str | None
    dependencies: list[str]
    runpaths: list[str]


def inspect_mach(path, kind):
    """Read load command metadata directly from the bytes in the archive."""
    with path.open("rb") as stream:
        data = stream.read(1024 * 1024 + 32)
    if len(data) < 32:
        raise ValueError("truncated Mach-O")
    magic, cpu, _, actual_kind, count, size, flags, _ = struct.unpack_from("<8I", data)
    if (magic, cpu, actual_kind) != (0xFEEDFACF, 0x0100000C, kind):
        raise ValueError("expected thin arm64 Mach-O executable or dylib for its role")
    end = 32 + size
    if end > len(data) or count > size // 8:
        raise ValueError("invalid Mach-O command bounds")
    offset, minimum, identity, entry = 32, None, None, False
    deps, rpaths = [], []
    for _ in range(count):
        if offset + 8 > end:
            raise ValueError("truncated load command")
        cmd, length = struct.unpack_from("<2I", data, offset)
        if length < 8 or length % 8 or offset + length > end:
            raise ValueError("malformed load command")
        if cmd == 0x32:
            if length < 24 or minimum is not None:
                raise ValueError("malformed build version")
            platform, minimum = struct.unpack_from("<2I", data, offset + 8)
            if platform != 2:
                raise ValueError("image is not iOS device code")
        elif cmd == 0x25:
            if length < 16 or minimum is not None:
                raise ValueError("malformed legacy deployment version")
            minimum = struct.unpack_from("<I", data, offset + 8)[0]
        elif cmd == 0x80000028:
            if length != 24 or entry:
                raise ValueError("malformed LC_MAIN")
            entryoff = struct.unpack_from("<Q", data, offset + 8)[0]
            if not 0 < entryoff < path.stat().st_size:
                raise ValueError("invalid LC_MAIN entry offset")
            entry = True
        elif cmd in {0xC, 0xD, 0x80000018, 0x8000001C, 0x8000001F, 0x20, 0x80000023}:
            floor = 12 if cmd == 0x8000001C else 24
            if length <= floor:
                raise ValueError("short dylib or runpath command")
            start = struct.unpack_from("<I", data, offset + 8)[0]
            if not floor <= start < length:
                raise ValueError("invalid load command string offset")
            value, sep, _ = data[offset + start:offset + length].partition(b"\0")
            if not sep or not value:
                raise ValueError("invalid load command string")
            name = value.decode("utf-8")
            if cmd == 0xD:
                if identity is not None:
                    raise ValueError("duplicate dylib identity")
                identity = name
            elif cmd == 0x8000001C:
                rpaths.append(name)
            else:
                deps.append(name)
        offset += length
    if offset != end or minimum != 15 << 16 or (kind == 2 and (not entry or not flags & 0x200000)):
        raise ValueError("wrong deployment target, entry point or PIE flag")
    return Image(minimum, identity, deps, rpaths)


def allowed(relative, executable, framework):
    parts = relative.parts
    if len(parts) == 1:
        return parts[0] in {"Info.plist", "PkgInfo", executable, "Assets.car"} or bool(
            re.fullmatch(r"AppIcon[0-9x@~a-zA-Z_-]*\.png", parts[0]))
    if parts[:2] != ("Frameworks", framework + ".framework"):
        return False
    if len(parts) == 3:
        return parts[2] in {"Info.plist", framework, "Assets.car"}
    return (len(parts) == 5 and parts[2:4] == ("Modules", framework + ".swiftmodule")
            and re.fullmatch(r"arm64[e]?(?:-apple-ios)?\.(?:swiftmodule|swiftdoc|swiftsourceinfo|swiftinterface|private\.swiftinterface|abi\.json)", parts[4]) is not None)


def archive_entries(zip_file, app_name, executable, framework):
    root = ("Payload", app_name + ".app")
    files, seen, total = {}, set(), 0
    for item in zip_file.infolist():
        raw = item.filename.removesuffix("/")
        path = PurePosixPath(raw)
        if raw != path.as_posix() or path.is_absolute() or ".." in path.parts or "\\" in raw:
            raise ValueError("unsafe or ambiguous archive path")
        key = raw.casefold()
        if key in seen:
            raise ValueError("duplicate archive path")
        seen.add(key)
        mode = item.external_attr >> 16
        kind = stat.S_IFMT(mode)
        if item.is_dir():
            if kind not in (stat.S_IFDIR, 0):
                raise ValueError("invalid archive directory type")
            if path.parts not in {("Payload",), root, root + ("Frameworks",),
                                   root + ("Frameworks", framework + ".framework"),
                                   root + ("Frameworks", framework + ".framework", "Modules"),
                                   root + ("Frameworks", framework + ".framework", "Modules", framework + ".swiftmodule")}:
                raise ValueError("unexpected archive directory: " + raw)
            continue
        if kind not in (stat.S_IFREG, 0) or item.flag_bits & 1:
            raise ValueError("linked, special or encrypted archive member")
        relative = PurePosixPath(*path.parts[2:])
        if path.parts[:2] != root or not allowed(relative, executable, framework):
            raise ValueError("unexpected archive member: " + raw)
        total += item.file_size
        if item.file_size > MAX_BYTES or total > MAX_BYTES or len(files) >= 128:
            raise ValueError("archive exceeds bounded inspection budget")
        files[relative] = item
    for path in (PurePosixPath(executable), PurePosixPath("Frameworks", framework + ".framework", framework)):
        if path not in files or not files[path].external_attr >> 16 & stat.S_IXUSR:
            raise ValueError("missing executable or framework executable mode")
    return files


def read_info(path):
    if path.stat().st_size > 128 * 1024:
        raise ValueError("oversized Info.plist")
    with path.open("rb") as stream:
        return plistlib.load(stream)


def verify_bundle(folder, profile, source, expected_app_digest, expected_framework_digest):
    app_name, executable, framework, bundle_id = profile
    fw = folder / "Frameworks" / (framework + ".framework")
    binary, library = folder / executable, fw / framework
    info, fi = read_info(folder / "Info.plist"), read_info(fw / "Info.plist")
    required = {
        "CFBundleExecutable": executable, "CFBundleIdentifier": bundle_id, "CFBundlePackageType": "APPL",
        "MinimumOSVersion": "15.0", "UIDeviceFamily": [1, 2], "UIFileSharingEnabled": True,
        "LSSupportsOpeningDocumentsInPlace": True, "HeliumDiagnosticSource": source,
    }
    if any(info.get(k) != v for k, v in required.items()):
        raise ValueError("app plist identity, Files access or platform mismatch")
    if not all(re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,2}", str(info.get(key, "")))
               for key in ("CFBundleVersion", "CFBundleShortVersionString")):
        raise ValueError("missing or invalid build/version")
    oriented = info.get("UISupportedInterfaceOrientations~ipad", info.get("UISupportedInterfaceOrientations"))
    if not isinstance(oriented, list) or not oriented or not set(oriented) <= ORIENTATIONS:
        raise ValueError("missing or invalid orientations")
    manifest = info.get("UIApplicationSceneManifest")
    if (not isinstance(manifest, dict) or manifest.get("UIApplicationSupportsMultipleScenes") is not False
            or "UILaunchScreen" not in info or "UIMainStoryboardFile" in info or "NSPrincipalClass" in info):
        raise ValueError("invalid programmatic launch or scene manifest")
    config = manifest.get("UISceneConfigurations", {}).get("UIWindowSceneSessionRoleApplication", [])
    if profile == GAME and (len(config) != 1 or config[0].get("UISceneDelegateClassName") != "WinitSceneDelegate"):
        raise ValueError("wrong Rust game scene delegate")
    if profile == GAME and (type(info.get("HeliumDisposableWorldTestVersion")) is not int
                            or info["HeliumDisposableWorldTestVersion"] != 1):
        raise ValueError("missing supported disposable world test marker")
    if profile == PROBE and (len(config) != 1 or config[0].get("UISceneDelegateClassName") != "HeliumProbeSceneDelegate" or info.get("HeliumNoWorldProbe") is not True):
        raise ValueError("wrong generated SwiftUI scene or probe marker")
    if not (folder / "Assets.car").is_file() or not info.get("CFBundleIcons"):
        raise ValueError("missing compiled app icon resources")
    expected_framework_id = "dev.helium.metalprobe.launcher" if profile == PROBE else "dev.horizon.game.launcher"
    if (fi.get("CFBundleExecutable") != framework or fi.get("CFBundlePackageType") != "FMWK"
            or fi.get("CFBundleIdentifier") != expected_framework_id or fi.get("MinimumOSVersion") != "15.0"):
        raise ValueError("framework plist mismatch")
    if digest(binary) != expected_app_digest or digest(library) != expected_framework_digest:
        raise ValueError("archive executable identity differs from verified build")
    app, dylib = inspect_mach(binary, 2), inspect_mach(library, 6)
    install_name = "@rpath/" + framework + ".framework/" + framework
    if dylib.id != install_name or app.id is not None:
        raise ValueError("invalid framework install name")
    if profile == PROBE and (install_name not in app.dependencies or "@executable_path/Frameworks" not in app.runpaths):
        raise ValueError("app cannot resolve the embedded probe launcher")
    for image in (app, dylib):
        for rpath in image.runpaths:
            if rpath not in {"@loader_path/Frameworks", "@executable_path/Frameworks", "/usr/lib/swift"}:
                raise ValueError("unexpected runpath: " + rpath)
        for dep in image.dependencies:
            if dep == install_name and image is app:
                continue
            if dep.startswith(("/System/Library/Frameworks/", "/usr/lib/")) and ".." not in PurePosixPath(dep).parts:
                continue
            # Apple Swift overlays are provided by iOS or the system Swift runpath.
            if re.fullmatch(r"@rpath/libswift[A-Za-z0-9_]+\.dylib", dep) and "/usr/lib/swift" in image.runpaths + app.runpaths:
                continue
            raise ValueError("unresolved/non-system image: " + dep)
    return info


def apple_checks(folder, profile):
    _, executable, framework, _ = profile
    library = folder / "Frameworks" / (framework + ".framework") / framework
    for path in (folder / executable, library):
        subprocess.run(["xcrun", "otool", "-hV", "-L", str(path)], check=True)
        signing = subprocess.run(["codesign", "-dv", "--verbose=4", str(path)], capture_output=True, text=True)
        message = signing.stdout + signing.stderr
        if "Authority=" in message or not ("not signed at all" in message or "Signature=adhoc" in message):
            raise ValueError("expected no distribution signature")
        if profile == PROBE:
            symbols = subprocess.check_output(["xcrun", "nm", str(path)], text=True)
            if any(s in symbols for s in ("horizon_client", "horizon_world", "wgpu_core", "_horizon_launcher_present")):
                raise ValueError("world/game or wgpu symbol in independent probe")
            if path == library and not all(s in symbols for s in (
                "_helium_display_run", "_helium_display_status", "_helium_display_progress",
                "_OBJC_CLASS_$_HeliumProbeSceneDelegate",
            )):
                raise ValueError("missing Rust probe bridge or UIKit scene delegate")
        elif path == library:
            symbols = subprocess.check_output(["xcrun", "nm", "-g", str(path)], text=True)
            if not all(s in symbols for s in ("_helium_checkpoint_begin", "_helium_checkpoint_end")):
                raise ValueError("missing game checkpoint bridge")


def verify_ipa(ipa, profile, source, app_digest, framework_digest, *, apple=False):
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("source must be the exact git SHA")
    name, executable, framework, bundle = profile
    with zipfile.ZipFile(ipa) as archive, tempfile.TemporaryDirectory(prefix="helium-verify-") as temporary:
        folder = Path(temporary)
        items = archive_entries(archive, name, executable, framework)
        for path, item in items.items():
            target = folder.joinpath(*path.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(item) as src, target.open("xb") as dst:
                while chunk := src.read(128 * 1024):
                    dst.write(chunk)
            target.chmod(0o755 if item.external_attr >> 16 & stat.S_IXUSR else 0o644)
        info = verify_bundle(folder, profile, source, app_digest, framework_digest)
        if apple:
            apple_checks(folder, profile)
    return {
        "file": ipa.name, "ipa_sha256": digest(ipa), "executable_sha256": app_digest,
        "framework_sha256": framework_digest, "bundle_id": bundle, "framework": framework,
        "version": info["CFBundleShortVersionString"], "build_number": info["CFBundleVersion"],
        "source": source, "verification": "PASS", "signing": "unsigned or linker-only ad-hoc",
        "hardware": "NOT TESTED", "purpose": "NO-WORLD DEVICE PROBE" if profile == PROBE else "HELD GAME STRUCTURE CHECK",
    }


def build_archive(app, ipa, profile):
    name, _, _, _ = profile
    if app.name != name + ".app":
        raise ValueError("wrong Xcode product name")
    with zipfile.ZipFile(ipa, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for item in sorted(app.rglob("*")):
            if item.is_symlink():
                raise ValueError("unexpected symlink in iOS flat bundle")
            if item.is_dir():
                continue
            archive.write(item, (PurePosixPath("Payload", app.name) / item.relative_to(app)).as_posix())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=["probe", "game"])
    parser.add_argument("app", type=Path)
    parser.add_argument("ipa", type=Path)
    parser.add_argument("source")
    args = parser.parse_args()
    profile = PROBE if args.profile == "probe" else GAME
    _, executable, framework, _ = profile
    build_archive(args.app, args.ipa, profile)
    try:
        result = verify_ipa(args.ipa, profile, args.source, digest(args.app / executable),
                            digest(args.app / "Frameworks" / (framework + ".framework") / framework), apple=True)
    except Exception:
        args.ipa.unlink()
        raise
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
