"""Package this freestanding ARM ELF as a Switch NRO (no external libraries)."""
import hashlib
import struct
import sys
from pathlib import Path


def convert(data):
    if len(data) < 52 or data[:7] != b'\x7fELF\x01\x01\x01' or struct.unpack_from('<H', data, 18)[0] != 40:
        raise ValueError('expected a little-endian ARM32 ELF')
    phoff = struct.unpack_from('<I', data, 28)[0]
    phsize, phnum = struct.unpack_from('<HH', data, 42)
    if phsize != 32 or phoff + phsize * phnum > len(data):
        raise ValueError('invalid program headers')
    segments = []
    for i in range(phnum):
        typ, off, va, _, size, mem, flags, _ = struct.unpack_from('<8I', data, phoff + i * phsize)
        if typ == 1:
            if off + size > len(data) or mem < size or va % 4096:
                raise ValueError('invalid load segment')
            segments.append((off, va, size, mem, flags))
    if len(segments) != 3 or [s[4] for s in segments] != [5, 4, 6]:
        raise ValueError('expected RX, R, RW load segments')
    cursor = 0
    sizes = []
    for _, va, size, _, _ in segments:
        if va != cursor:
            raise ValueError('non-contiguous load segments')
        size = (size + 4095) & ~4095
        sizes.append(size)
        cursor += size
    image = bytearray(cursor)
    for off, va, size, _, _ in segments:
        image[va:va+size] = data[off:off+size]
    mod = struct.unpack_from('<I', image, 4)[0]
    if image[mod:mod+4] != b'MOD0' or mod < 128:
        raise ValueError('missing MOD0 or reserved header space')
    bss = max(0, (segments[2][3] - sizes[2] + 4095) & ~4095)
    header = bytearray(112)
    header[:4] = b'NRO0'
    struct.pack_into('<I', header, 8, cursor)
    for i, seg in enumerate(segments):
        struct.pack_into('<II', header, 16+i*8, seg[1], sizes[i])
    struct.pack_into('<I', header, 40, bss)
    header[48:80] = hashlib.sha256(data).digest()
    image[16:128] = header
    return bytes(image)


if __name__ == '__main__':
    Path(sys.argv[2]).write_bytes(convert(Path(sys.argv[1]).read_bytes()))
