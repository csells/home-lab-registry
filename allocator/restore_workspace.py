"""Install a verified checkpoint in the task user's new Linux workspace."""
import hashlib
import json
import os
import re
import sys
import tarfile
import tempfile
from pathlib import Path

MAX_ARCHIVE = 64 * 1024 ** 2
MAX_EXPANDED = 512 * 1024 ** 2
MAX_MEMBERS = 20000


def restore(digest, lease):
    if not re.fullmatch('[a-f0-9]{64}', digest) or not re.fullmatch('[a-f0-9]{32}', lease):
        raise ValueError('Invalid restore identity.')
    home = Path.home()
    workspace = home / 'workspace'
    marker = '.lease-restore-receipt'
    identity = {'sha256': digest, 'lease': lease}
    os.umask(0o077)
    with tempfile.TemporaryDirectory(dir=home) as tmp:
        target = Path(tmp) / 'workspace'
        target.mkdir()
        checksum = hashlib.sha256()
        with tempfile.TemporaryFile(dir=tmp) as source:
            while block := sys.stdin.buffer.read(65536):
                if source.tell() + len(block) > MAX_ARCHIVE:
                    raise ValueError('Recovery archive exceeds the supported size.')
                source.write(block)
                checksum.update(block)
            if checksum.hexdigest() != digest:
                raise ValueError('Downloaded checkpoint failed verification in the guest.')
            if workspace.is_symlink():
                raise ValueError('Existing workspace is a symbolic link.')
            if workspace.exists():
                receipt = workspace / marker
                if receipt.is_file() and not receipt.is_symlink() and receipt.stat().st_size < 256:
                    if json.loads(receipt.read_text()) == identity:
                        return 'previously-restored'
                if any(workspace.iterdir()):
                    raise ValueError('An existing workspace already contains data.')
            source.seek(0)
            size = count = 0
            with tarfile.open(fileobj=source, mode='r|gz') as archive:
                for entry in archive:
                    count += 1
                    size += entry.size
                    if count > MAX_MEMBERS or size > MAX_EXPANDED:
                        raise ValueError('Expanded checkpoint exceeds the workspace budget.')
                    # The restore receipt describes this generation and is regenerated.
                    if Path(entry.name) == Path(marker):
                        continue
                    if entry.isdir() or entry.isreg() or entry.issym() or entry.islnk():
                        archive.extract(entry, target, filter='data', set_attrs=True)
                    else:
                        raise ValueError('Unsupported file type in checkpoint.')
        with (target / marker).open('x') as receipt:
            json.dump(identity, receipt)
        # A nonempty destination cannot be replaced by a directory rename.
        target.rename(workspace)
    return 'restored'


if __name__ == '__main__':
    try:
        print(restore(*sys.argv[1:]))
    except (OSError, ValueError, tarfile.TarError) as error:
        sys.exit(f'Workspace restore failed: {error}')
