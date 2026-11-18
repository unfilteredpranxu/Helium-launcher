"""Structural archive fixtures. Apple binaries are verified separately on CI."""
import importlib.util
from pathlib import Path
import plistlib
import stat
import struct
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("ios_package", Path(__file__).parents[1] / "tools/ci/ios_package.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)
SOURCE = "1" * 40
NAME, EXE, KIT, BUNDLE = package.PROBE
ROOT = f"Payload/{NAME}.app/"
KIT_PATH = f"Frameworks/{KIT}.framework/"
INSTALL = f"@rpath/{KIT}.framework/{KIT}"


def load_string(cmd, name):
    floor = 12 if cmd == 0x8000001C else 24
    body = name.encode() + b"\0"
    size = (floor + len(body) + 7) & ~7
    return struct.pack("<3I", cmd, size, floor).ljust(floor, b"\0") + body.ljust(size - floor, b"\0")


def mach(kind, platform=2, dependency=INSTALL, rpath="@executable_path/Frameworks"):
    commands = [struct.pack("<6I", 0x32, 24, platform, 15 << 16, 0, 0)]
    if kind == 2:
        commands += [struct.pack("<IIQQ", 0x80000028, 24, 2048, 0),
                     load_string(0xC, dependency), load_string(0x8000001C, rpath)]
    else:
        commands += [load_string(0xD, INSTALL),
                     load_string(0xC, "/System/Library/Frameworks/Metal.framework/Metal")]
    return (struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, kind, len(commands),
                        sum(map(len, commands)), 0x200000, 0) + b"".join(commands)).ljust(4096, b"\0")


def metadata():
    return {
        "CFBundleExecutable": EXE, "CFBundleIdentifier": BUNDLE, "CFBundlePackageType": "APPL",
        "CFBundleVersion": "13001", "CFBundleShortVersionString": "0.1.0",
        "MinimumOSVersion": "15.0", "UIDeviceFamily": [1, 2], "UIFileSharingEnabled": True,
        "LSSupportsOpeningDocumentsInPlace": True, "HeliumDiagnosticSource": SOURCE,
        "HeliumNoWorldProbe": True, "UILaunchScreen": {},
        "CFBundleIcons": {"CFBundlePrimaryIcon": {"CFBundleIconName": "AppIcon"}},
        "UISupportedInterfaceOrientations": ["UIInterfaceOrientationPortrait"],
        "UIApplicationSceneManifest": {"UIApplicationSupportsMultipleScenes": False,
                                       "UISceneConfigurations": {"UIWindowSceneSessionRoleApplication":
                                           [{"UISceneDelegateClassName": "HeliumProbeSceneDelegate"}]}},
    }


class IpaContract(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.ipa = Path(temporary.name) / "fixture.ipa"
        self.entries = {
            EXE: mach(2), "Info.plist": plistlib.dumps(metadata()), "Assets.car": b"fixture",
            KIT_PATH + KIT: mach(6),
            KIT_PATH + "Info.plist": plistlib.dumps({
                "CFBundleExecutable": KIT, "CFBundleIdentifier": BUNDLE + ".launcher",
                "MinimumOSVersion": "15.0", "CFBundlePackageType": "FMWK",
            }),
        }

    def verify(self, *, extra=None, app_hash=None, framework_hash=None, link=None):
        import hashlib
        with zipfile.ZipFile(self.ipa, "w") as archive:
            for name, data in self.entries.items():
                item = zipfile.ZipInfo(ROOT + name)
                mode = 0o755 if name in {EXE, KIT_PATH + KIT} else 0o644
                item.external_attr = (stat.S_IFREG | mode) << 16
                archive.writestr(item, data)
            if extra:
                archive.writestr(extra, b"invalid fixture")
            if link:
                item = zipfile.ZipInfo(ROOT + link)
                item.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(item, "temp")
        return package.verify_ipa(
            self.ipa, package.PROBE, SOURCE,
            app_hash or hashlib.sha256(self.entries[EXE]).hexdigest(),
            framework_hash or hashlib.sha256(self.entries.get(KIT_PATH + KIT, b"")).hexdigest(),
        )

    def test_valid_archive(self):
        result = self.verify()
        self.assertEqual((result["verification"], result["bundle_id"], result["hardware"]),
                         ("PASS", BUNDLE, "NOT TESTED"))

    def test_missing_framework_is_rejected(self):
        del self.entries[KIT_PATH + KIT]
        with self.assertRaisesRegex(ValueError, "missing executable or framework"):
            self.verify()

    def test_framework_mach_type_and_platform_are_rejected(self):
        for invalid in [mach(2), mach(6, platform=7), b"bad"]:
            self.entries[KIT_PATH + KIT] = invalid
            with self.assertRaises(ValueError):
                self.verify()

    def test_wrong_app_hash_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "identity differs"):
            self.verify(app_hash="0" * 64)

    def test_wrong_framework_hash_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "identity differs"):
            self.verify(framework_hash="0" * 64)

    def test_missing_framework_runpath_is_rejected(self):
        self.entries[EXE] = mach(2, rpath="/usr/lib/swift")
        with self.assertRaisesRegex(ValueError, "cannot resolve"):
            self.verify()

    def test_unresolved_embedded_dependency_is_rejected(self):
        self.entries[KIT_PATH + KIT] += b"altered"
        self.entries[EXE] = mach(2, dependency="@rpath/Other.framework/Other")
        with self.assertRaisesRegex(ValueError, "cannot resolve"):
            self.verify()

    def test_wrong_scene_id_or_missing_files_access_is_rejected(self):
        for patch in [{"CFBundleIdentifier": "dev.horizon.game"}, {"UIFileSharingEnabled": False},
                      {"UIApplicationSceneManifest": {}}]:
            info = metadata()
            info.update(patch)
            self.entries["Info.plist"] = plistlib.dumps(info)
            with self.assertRaises(ValueError):
                self.verify()

    def test_save_provision_duplicate_and_escape_are_rejected(self):
        for unexpected in [ROOT + "level.dat", ROOT + "embedded.mobileprovision",
                           ROOT + EXE.upper(), ROOT + "../sneaky", "SwiftSupport/lib.dylib"]:
            with self.assertRaises(ValueError):
                self.verify(extra=unexpected)

    def test_symlink_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "linked"):
            self.verify(link="PkgInfo")
