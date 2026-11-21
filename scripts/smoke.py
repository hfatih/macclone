"""End-to-end tests through the real CLI, using only temporary regular files."""
import argparse
import hashlib
import json
import subprocess
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
        # Damage the primary table, then the secondary table. Inspection never repairs either.
        raw[2*sector + 7] ^= 1
        src.write_bytes(raw)
        assert 'Using backup GPT' in cli('inspect', src, *settings)['warnings'][0]
        raw[-sector+16] ^= 1
        src.write_bytes(raw)
        cli('inspect', src, *settings, success=False)
        print(f'{sector} B: GPT/EFI/APFS, dry-run, full clone, SHA-256, resume, refusal, backup fallback and both CRC failures passed')
