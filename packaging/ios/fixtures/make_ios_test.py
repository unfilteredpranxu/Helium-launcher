"""Builds ios_test-1.1.0.hmod, the content mod used to test the iOS Mods page."""

import json
import os
import struct
import zipfile
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ios_test-1.1.0.hmod")

MANIFEST = """id = "ios_test"
name = "iOS Test Content"
version = "1.1.0"
description = "A glowing ruby block, an item and a mob effect for testing Horizon's Mods page on iPhone."
authors = ["Horizon iOS"]
api = "0.2"
side = "both"
capabilities = ["registries"]
"""

COLORS = {
    "K": (61, 5, 18, 255),
    "H": (255, 110, 140, 255),
    "B": (240, 48, 95, 255),
    "R": (193, 18, 63, 255),
    "D": (139, 10, 36, 255),
    "M": (165, 15, 49, 255),
    ".": (0, 0, 0, 0),
}

# A 16x16 ruby, cut like the diamond item.
ITEM = [
    "................",
    "................",
    ".......KK.......",
    "......KHHK......",
    ".....KHHBRK.....",
    "....KHHBRRRK....",
    "...KHHBRRRRDK...",
    "..KHHBRRRRRRDK..",
    "..KMHBRRRRRRDK..",
    "...KMBRRRRRDK...",
    "....KMBRRRDK....",
    ".....KMBRDK.....",
    "......KMDK......",
    ".......KK.......",
    "................",
    "................",
]

# One 8x8 beveled panel, tiled 2x2.
PANEL = [
    "KKKKKKKK",
    "KHHBBBBK",
    "KHBBBRRK",
    "KBBRRRRK",
    "KBRRRRDK",
    "KBRRRDDK",
    "KBRRDDDK",
    "KKKKKKKK",
]


def grid(rows):
    assert all(len(row) == 16 for row in rows) and len(rows) == 16
    return [[COLORS[c] for c in row] for row in rows]


def block_grid():
    rows = []
    for y in range(16):
        rows.append("".join(PANEL[y % 8][x % 8] for x in range(16)))
    return grid(rows)


def png(pixels):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + b"".join(bytes(px) for px in row) for row in pixels)
    header = struct.pack(">IIBBBBB", 16, 16, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


JSON_FILES = {
    # The block and the item keep vanilla behaviour, but now with their own
    # textures, models and blockstate.
    "data/ios_test/horizon/block/ruby_block.json": {
        "template": "minecraft:iron_block",
        "properties": {
            "hardness": 4.0,
            "light": 15,
            "sounds": "minecraft:amethyst_block",
        },
    },
    "data/ios_test/horizon/item/ruby.json": {
        "template": "minecraft:diamond",
        "rarity": "epic",
    },
    "data/ios_test/horizon/mob_effect/glow.json": {
        "category": "beneficial",
        "color": 8454143,
    },
    "assets/ios_test/items/ruby_block.json": {
        "model": {"type": "minecraft:model", "model": "ios_test:block/ruby_block"}
    },
    "assets/ios_test/items/ruby.json": {
        "model": {"type": "minecraft:model", "model": "ios_test:item/ruby"}
    },
    "assets/ios_test/blockstates/ruby_block.json": {
        "variants": {"": {"model": "ios_test:block/ruby_block"}}
    },
    "assets/ios_test/models/block/ruby_block.json": {
        "parent": "minecraft:block/cube_all",
        "textures": {"all": "ios_test:block/ruby_block"},
    },
    "assets/ios_test/models/item/ruby.json": {
        "parent": "minecraft:item/generated",
        "textures": {"layer0": "ios_test:item/ruby"},
    },
    "assets/ios_test/lang/en_us.json": {
        "block.ios_test.ruby_block": "Test Ruby Block",
        "item.ios_test.ruby": "Test Ruby",
        "effect.ios_test.glow": "Test Glow",
    },
}

PNG_FILES = {
    "assets/ios_test/textures/item/ruby.png": png(grid(ITEM)),
    "assets/ios_test/textures/block/ruby_block.png": png(block_grid()),
}


def main():
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as z:
        entries = {"horizon-mod.toml": MANIFEST.encode()}
        for name, value in JSON_FILES.items():
            entries[name] = (json.dumps(value, indent=2) + "\n").encode()
        entries.update(PNG_FILES)
        for name, data in entries.items():
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    print(OUT, os.path.getsize(OUT), "bytes")


if __name__ == "__main__":
    main()
