#!/bin/bash
# Standalone probe only. Produces an unsigned IPA for subsequent signing.
set -euo pipefail
probe_ios_dir=$(cd "$(dirname "$0")" && pwd)
command -v xcodegen >/dev/null
command -v xcodebuild >/dev/null
mkdir -p "$probe_ios_dir/build"
cd "$probe_ios_dir"
xcodegen generate --spec project.yml
xcodebuild -project HeliumMetalProbe.xcodeproj -scheme HeliumMetalProbe \
    -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
    -derivedDataPath "$probe_ios_dir/build/DerivedData" \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY='' build
probe_app="$probe_ios_dir/build/DerivedData/Build/Products/Release-iphoneos/HeliumMetalProbe.app"
test -d "$probe_app"
# Use a fresh staging directory/archive, so removed files cannot survive a zip update.
probe_stage=$(mktemp -d "$probe_ios_dir/build/ipa.XXXXXX")
mkdir "$probe_stage/Payload"
ditto "$probe_app" "$probe_stage/Payload/HeliumMetalProbe.app"
ditto -c -k --keepParent "$probe_stage/Payload" "$probe_stage/HeliumMetalProbe-unsigned.ipa"
mv "$probe_stage/HeliumMetalProbe-unsigned.ipa" "$probe_ios_dir/build/HeliumMetalProbe-unsigned.ipa"
echo "Unsigned probe IPA: $probe_ios_dir/build/HeliumMetalProbe-unsigned.ipa"
