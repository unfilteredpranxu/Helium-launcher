#!/usr/bin/env python3
"""Compile/test the typed CPU adapter against both existing locked Cargo graphs.

No new Cargo dependency/manifest/lockfile, runtime assets, GPU or game process.
MIT; Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile

EXPERIMENT = Path(__file__).resolve().parents[1]
REPOSITORY = EXPERIMENT.parents[1]
ENV = dict(os.environ, CARGO_BUILD_JOBS="2", CARGO_NET_OFFLINE="true")


def run(command, **kwargs):
    print("$", shlex.join(map(str, command)), flush=True)
    result = subprocess.run(command, cwd=REPOSITORY, env=ENV, **kwargs)
    print("exit:", result.returncode, flush=True)
    result.check_returncode()
    return result


def artifacts(manifest, package=None):
    command = ["cargo", "+1.95.0", "build", "--locked", "--offline", "--lib",
               "--manifest-path", str(manifest), "--message-format=json"]
    if package:
        command += ["-p", package]
    # Capture JSON without printing large dependency metadata; retain diagnostics.
    print("$", shlex.join(command), flush=True)
    result = subprocess.run(command, cwd=REPOSITORY, env=ENV, capture_output=True, text=True)
    found = {}
    for line in result.stdout.splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-message":
            print(event["message"].get("rendered", ""), end="", flush=True)
        if event.get("reason") == "compiler-artifact":
            for filename in event["filenames"]:
                if filename.endswith(".rlib"):
                    found[event["target"]["name"]] = Path(filename)
    print(result.stderr, end="", flush=True)
    print("exit:", result.returncode, flush=True)
    result.check_returncode()
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resources", type=Path, help="Also run the ignored authentic mesher test against owner-provided prepared vanilla resources")
    args = parser.parse_args()
    production = artifacts(REPOSITORY / "Cargo.toml", "horizon-client-render")
    experimental = artifacts(EXPERIMENT / "Cargo.toml")
    dependencies = {"horizon_client_render": production["horizon_client_render"],
                    "bytemuck": production["bytemuck"], "horizon_core": production["horizon_core"],
                    "helium_metal_prototype": experimental["helium_metal_prototype"]}
    with tempfile.TemporaryDirectory(prefix="helium-section-bridge-") as directory:
        binary = Path(directory) / "section-adapter-tests"
        command = ["rustc", "+1.95.0", "--edition=2024", "--test", "-C", "debuginfo=0",
                   str(EXPERIMENT / "tools/section_adapter.rs"), "-o", str(binary)]
        directories = {p.parent for p in dependencies.values()}
        # Top-level Cargo library artifacts also resolve transitive crates from
        # deps/, including the experiment's Apple-only objc2 dependencies on Macs.
        directories |= {p / "deps" for p in directories if (p / "deps").is_dir()}
        for parent in sorted(directories):
            command += ["-L", "dependency=" + str(parent)]
        for name, path in dependencies.items():
            command += ["--extern", name + "=" + str(path)]
        run(command)
        run([str(binary)])
        if args.resources:
            ENV["HORIZON_MESH_TEST_RESOURCES"] = str(args.resources.resolve())
            run([str(binary), "--ignored", "--exact",
                 "tests::actual_mesher_output_with_adjacent_cubes_fractional_models_and_tint"])


if __name__ == "__main__":
    main()
