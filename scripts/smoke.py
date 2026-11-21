"""End-to-end tests through the real CLI, using only temporary regular files."""
import argparse
import hashlib
import json
import struct
import subprocess
import zlib
import tempfile
from pathlib import Path
from make_fixture import make

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('macclone', type=Path)
parser.add_argument('--partition-manager', type=Path, metavar='PM',
                    help='compare RAW output against the pinned partition-manager CLI')
args = parser.parse_args()
exe = str(args.macclone.resolve())

def cli(*args, success=True):
    result = subprocess.run([exe, *map(str,args)], capture_output=True, text=True)
    if success:
        if result.returncode:
            raise RuntimeError(result.stderr)
        return json.loads(result.stdout)
    assert result.returncode != 0

with tempfile.TemporaryDirectory() as root:
    base = Path(root)
    for sector in (512,4096):
        folder = base/str(sector)
        folder.mkdir()
        src, job = folder/'mac.raw', folder/'job'
        # Cross both engines' transfer boundaries, with one final sector.
        raw = make(sector, sectors=8 * 1024 * 1024 // sector + 1)
        src.write_bytes(raw)
        settings = ['--sector', sector]
        layout = cli('inspect', src, *settings)
        assert layout['table'] == 'GPT' and not layout['warnings']
        assert [p['kind'] for p in layout['partitions']][0] == 'EFI System'
        assert layout['partitions'][1]['filesystem'] == 'APFS signature'
        assert cli('dry-run', src, *settings)['writes'] == 0 and not job.exists()
        report = cli('image', src, job, *settings)
        assert (job/'image.raw').read_bytes() == raw == src.read_bytes()
        assert report['sha256'] == hashlib.sha256(raw).hexdigest()
        assert cli('verify', job/'image.raw', report['sha256'], *settings)['outcome'] == 'verified'
        cli('verify', job/'image.raw', report['sha256'], *settings, '--resume', success=False)
        cli('inspect', src, *settings, '--block', 512, success=False)
        cli('image', src, folder/'duplicate-flags', *settings, '--sector', sector, success=False)
        if args.partition_manager:
            reference = folder/'pm-clone.raw'
            reference.write_bytes(b'\xa5' * len(raw))
            subprocess.run([str(args.partition_manager.resolve()), '--disk-file', str(src),
                            'clone', '--sector', 'file:'+str(src), 'file:'+str(reference)], check=True)
            assert reference.read_bytes() == raw == (job/'image.raw').read_bytes()
            assert src.read_bytes() == raw
            print(f'{sector} B GPT layout: both engines match all {len(raw)} source bytes')
        assert report == cli('image', src, job, '--resume', *settings)
        cli('image', src, job, *settings, success=False)
        old_backup = struct.unpack_from('<Q', raw, len(raw)-sector+72)[0] * sector
        for extra_sectors in (1, 65):
            larger = folder / f'larger-{extra_sectors}'
            size = len(raw) + extra_sectors * sector
            expanded = cli('image', src, larger, *settings, '--target-bytes', size)
            result = (larger/'image.raw').read_bytes()
            assert size == len(result) == expanded['target_reread_bytes']
            assert expanded['gpt_relocated_from_bytes'] == len(raw)
            assert expanded['sha256'] == hashlib.sha256(result).hexdigest()
            assert result[sector*2:old_backup] == raw[sector*2:old_backup]
            new_backup = old_backup + extra_sectors * sector
            assert result[old_backup:new_backup] == bytes(new_backup-old_backup)
            assert result[new_backup:-sector] == raw[old_backup:-sector]
            assert struct.unpack_from('<I', result, 458)[0] == size // sector - 1
            for offset in (sector, size-sector):
                header = bytearray(result[offset:offset+92])
                expected = struct.unpack_from('<I', header, 16)[0]
                header[16:20] = bytes(4)
                assert zlib.crc32(header) == expected
            target_layout = cli('inspect', larger/'image.raw', *settings)
            assert target_layout['partitions'] == layout['partitions']
            assert not target_layout['warnings']
            assert expanded == cli('image', src, larger, *settings, '--target-bytes', size, '--resume')
            cli('image', src, larger, *settings, '--target-bytes', size+sector, '--resume', success=False)
        small = folder/'smaller'
        cli('image', src, small, *settings, '--target-bytes', len(raw)-sector, success=False)
        assert not small.exists()
        hybrid = folder/'hybrid.raw'
        hybrid_data = raw[:]
        hybrid_data[466] = 0xaf
        hybrid.write_bytes(hybrid_data)
        cli('image', hybrid, folder/'hybrid-job', *settings, '--target-bytes', len(raw)+sector, success=False)
        assert not (folder/'hybrid-job').exists()
        assert src.read_bytes() == raw
        backup_only = folder/'backup-only.raw'
        missing_front = raw[:]
        missing_front[:2*sector] = bytes(2*sector)
        backup_only.write_bytes(missing_front)
        recovered = cli('inspect', backup_only, *settings)
        assert recovered['table'] == 'GPT'
        assert recovered['partitions'] == layout['partitions']
        assert any('Using backup GPT' in warning for warning in recovered['warnings'])
        assert 'Protective MBR missing' in recovered['warnings']

        shared = folder/'shared-array.raw'
        shared_data = raw[:]
        header_offset = len(shared_data)-sector
        struct.pack_into('<Q', shared_data, header_offset+72, 2)
        struct.pack_into('<I', shared_data, header_offset+16, 0)
        struct.pack_into('<I', shared_data, header_offset+16, zlib.crc32(shared_data[header_offset:header_offset+92]))
        shared.write_bytes(shared_data)
        shared_layout = cli('inspect', shared, *settings)
        assert any('Backup GPT at final LBA' in warning for warning in shared_layout['warnings'])
        # Damage the primary table, then the secondary table. Inspection never repairs either.
        raw[2*sector + 7] ^= 1
        src.write_bytes(raw)
        assert 'Using backup GPT' in cli('inspect', src, *settings)['warnings'][0]
        cli('image', src, folder/'bad-gpt', *settings, '--target-bytes', len(raw)+sector, success=False)
        assert not (folder/'bad-gpt').exists()
        raw[-sector+16] ^= 1
        src.write_bytes(raw)
        cli('inspect', src, *settings, success=False)
        print(f'{sector} B: GPT/EFI/APFS, dry-run, full clone, SHA-256, resume, refusal, larger GPT relocation and backup CRC checks passed')
