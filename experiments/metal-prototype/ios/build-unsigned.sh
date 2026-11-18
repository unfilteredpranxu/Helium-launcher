#!/bin/bash
# Build and verify the standalone probe only. No game or signing step.
set -euo pipefail
probe_ios_dir=$(cd "$(dirname "$0")" && pwd)
probe_repo=$(git -C "$probe_ios_dir" rev-parse --show-toplevel)
probe_sha=$(git -C "$probe_repo" rev-parse HEAD)
test -z "$(git -C "$probe_repo" status --porcelain)" || {
    echo 'error: use a clean committed checkout so the IPA identity is truthful.' >&2
    exit 1
}
command -v xcodegen >/dev/null
command -v xcodebuild >/dev/null
mkdir -p "$probe_ios_dir/build"
cd "$probe_ios_dir"
xcodegen generate --spec project.yml
env -u MACOSX_DEPLOYMENT_TARGET xcodebuild -project HeliumMetalProbe.xcodeproj -scheme HeliumMetalProbe \
    -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
    -derivedDataPath "$probe_ios_dir/build/DerivedData" \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY='' \
    HELIUM_DIAGNOSTIC_SHA="$probe_sha" build
probe_app="$probe_ios_dir/build/DerivedData/Build/Products/Release-iphoneos/HeliumMetalProbe.app"
probe_stage=$(mktemp -d "$probe_ios_dir/build/ipa.XXXXXX")
trap 'rm -rf "$probe_stage"' EXIT
probe_ipa=Helium-S1H-Metal-Probe-unsigned.ipa
python3 "$probe_repo/tools/ci/ios_package.py" probe "$probe_app" "$probe_stage/$probe_ipa" "$probe_sha" \
    | tee "$probe_stage/verification.txt"
mv "$probe_stage/$probe_ipa" "$probe_ios_dir/build/$probe_ipa"
mv "$probe_stage/verification.txt" "$probe_ios_dir/build/verification.txt"
