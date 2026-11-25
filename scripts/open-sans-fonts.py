#!/usr/bin/env python3
"""Write the Open Sans fonts the egui interface embeds (regular, semibold, bold) as TrueType files.

Usage: python3 scripts/open-sans-fonts.py <@fontsource/open-sans folder>. Get the package with
`npm install --prefix /tmp/fonts @fontsource/open-sans@5.1.0`. Reads its WOFF files and unpacks them; egui reads TrueType and OpenType, not WOFF. Open Sans is under the SIL Open Font
License 1.1, copied next to the fonts.
"""
import shutil
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "node_modules/@fontsource/open-sans"
TARGET = ROOT / "crates/gitcraft/assets/fonts"


def woff_to_sfnt(data: bytes) -> bytes:
    signature, flavor, _length, num_tables = struct.unpack(">4sIIH", data[:14])
    if signature != b"wOFF":
        raise ValueError("not a WOFF file")
    tables = []
    for i in range(num_tables):
        tag, offset, comp_length, orig_length, checksum = struct.unpack(">4sIIII", data[44 + i * 20:64 + i * 20])
        raw = data[offset:offset + comp_length]
        if comp_length < orig_length:
            raw = zlib.decompress(raw)
        tables.append((tag, checksum, raw))
    tables.sort(key=lambda t: t[0])
    power = 1
    while power * 2 <= num_tables:
        power *= 2
    search_range = power * 16
    entry_selector = power.bit_length() - 1
    range_shift = num_tables * 16 - search_range
    out = bytearray(struct.pack(">IHHHH", flavor, num_tables, search_range, entry_selector, range_shift))
    offset = 12 + 16 * num_tables
    records = bytearray()
    body = bytearray()
    for tag, checksum, raw in tables:
        records += struct.pack(">4sIII", tag, checksum, offset + len(body), len(raw))
        body += raw
        while len(body) % 4:
            body += b"\0"
    return bytes(out + records + body)


def main() -> int:
    TARGET.mkdir(parents=True, exist_ok=True)
    for weight, name in [(400, "OpenSans-Regular.ttf"), (600, "OpenSans-SemiBold.ttf"), (700, "OpenSans-Bold.ttf")]:
        woff = SOURCE / "files" / f"open-sans-latin-{weight}-normal.woff"
        (TARGET / name).write_bytes(woff_to_sfnt(woff.read_bytes()))
    shutil.copy(SOURCE / "LICENSE", TARGET / "LICENSE-OpenSans")
    return 0


if __name__ == "__main__":
    sys.exit(main())
