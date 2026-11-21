"""Generate a synthetic 2 MiB GPT disk with redundant tables and a probe signature.
The APFS partition has only an NXSB tag. It cannot be mounted or booted.
"""
import argparse
import hashlib
import struct
import uuid
import zlib
from pathlib import Path

def make(sector=512, sectors=None):
    if sectors is None:
        sectors = 2 * 1024 * 1024 // sector
    image = bytearray(sectors * sector)
    image[446 + 4] = 0xEE
    struct.pack_into('<II', image, 454, 1, sectors - 1)
    image[510:512] = b'\x55\xaa'
    entries = bytearray(128 * 128)
    table_sectors = len(entries) // sector
    first, last = 2 + table_sectors, sectors - 2 - table_sectors
    efi_start = first + 5
    efi_end = efi_start + 15
    mac_start = efi_end + 12
    mac_end = last - 7
    if mac_end < mac_start:
        raise ValueError("Fixture is too small for GPT and the test partitions")
    # Pattern the partitions AND all three unused gaps. A copier that skips gaps
    # will differ even when its newly created output was initialized to zeros.
    region = slice(first * sector, (last + 1) * sector)
    image[region] = hashlib.shake_256(b'MacClone whole-disk fixture').digest(region.stop - region.start)
    for i, (kind, name, begin, end) in enumerate([
        ('c12a7328-f81f-11d2-ba4b-00a0c93ec93b', 'EFI', efi_start, efi_end),
        ('7c3457ef-0000-11aa-aa11-00306543ecac', 'Macintosh', mac_start, mac_end),
    ]):
        base = i * 128
        entries[base:base+16] = uuid.UUID(kind).bytes_le
        entries[base+16:base+32] = uuid.UUID(int=i+1).bytes_le
        struct.pack_into('<QQ', entries, base+32, begin, end)
        label = name.encode('utf-16le')
        entries[base+56:base+56+len(label)] = label
    for current, alternate, array in [(1, sectors-1, 2), (sectors-1, 1, last+1)]:
        header = bytearray(sector)
        struct.pack_into('<8sIIIIQQQQ16sQIII', header, 0, b'EFI PART', 0x10000,
                         92, 0, 0, current, alternate, first, last,
                         uuid.UUID(int=3).bytes_le, array, 128, 128, zlib.crc32(entries))
        struct.pack_into('<I', header, 16, zlib.crc32(header[:92]))
        image[current*sector:(current+1)*sector] = header
        image[array*sector:array*sector+len(entries)] = entries
    offset = mac_start*sector
    image[offset+32:offset+36] = b'NXSB'
    return image

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('output', type=Path)
    parser.add_argument('--sector', type=int, choices=[512,4096], default=512)
    args = parser.parse_args()
    with args.output.open('xb') as stream:
        stream.write(make(args.sector))
