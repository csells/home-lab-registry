"""Install a verified checkpoint in the task user's declared workspace."""
import gzip
import hashlib
import json
import os
import re
import resource
import shutil
import sys
import tarfile
import tempfile
from pathlib import Path

MAX_ARCHIVE = 64 * 1024 ** 2
MAX_EXPANDED = 512 * 1024 ** 2
MAX_MEMBERS = 20000
APPLE_WORKSPACE = '/Volumes/My Shared Files/workspace/project'


def extract_workspace(source, target):
    """Exercise the same filesystem and tar policy before acknowledgment and on resume."""
    marker = '.lease-restore-receipt'
    # Charge gzip output before tarfile parses metadata, including long names and padding.
    with gzip.GzipFile(fileobj=source) as compressed, tempfile.TemporaryFile(dir=target.parent) as tar:
        while block := compressed.read(65536):
            if tar.tell() + len(block) > MAX_EXPANDED:
                raise ValueError('Expanded checkpoint exceeds the workspace budget.')
            tar.write(block)
        tar.seek(0)
        extract_tar(tar, target, marker)


def extract_tar(tar, target, marker):
    size = count = 0
    with tarfile.open(fileobj=tar, mode='r|') as archive:
        for entry in archive:
            count += 1
            size += entry.size
            if count > MAX_MEMBERS or size > MAX_EXPANDED:
                raise ValueError('Expanded checkpoint exceeds the workspace budget.')
            # The restore receipt describes this generation and is regenerated.
            if Path(entry.name) == Path(marker):
                continue
            if entry.isdir() or entry.isreg() or entry.issym() or entry.islnk():
                if hasattr(tarfile, 'data_filter'):
                    archive.extract(entry, target, filter='data', set_attrs=True)
                else:
                    extract_legacy_member(archive, entry, target)
            else:
                raise ValueError('Unsupported file type in checkpoint.')
    # Reserve this exact path before acknowledging that receipt installation can succeed.
    with (target / marker).open('x'):
        pass


def inside(root, path):
    if path.is_absolute() or not (root/path).resolve().is_relative_to(root):
        raise ValueError('Archive entry or link points outside the workspace.')
    return root/path


def extract_legacy_member(archive, entry, root):
    """Extract only data on guest Python versions predating tarfile filters."""
    root = root.resolve(strict=True)
    path = inside(root, Path(entry.name))
    if entry.isdir():
        if path.is_symlink():
            raise ValueError('Archive directory is redirected.')
        path.mkdir(parents=True, exist_ok=True)
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if entry.isreg():
        with archive.extractfile(entry) as source, path.open('xb') as output:
            shutil.copyfileobj(source, output, length=65536)
        path.chmod(0o600 | (entry.mode & 0o111))
    elif entry.issym():
        link = Path(entry.linkname)
        if link.is_absolute():
            raise ValueError('Archive link points outside the workspace.')
        inside(root, path.parent.relative_to(root)/link)
        path.symlink_to(link)
    elif entry.islnk():
        original = inside(root, Path(entry.linkname))
        if original.is_symlink() or not original.is_file():
            raise ValueError('Archive hard link has no earlier regular source.')
        os.link(original,path)


def restore(digest, lease, destination=None):
    if not re.fullmatch('[a-f0-9]{64}', digest) or not re.fullmatch('[a-f0-9]{32}', lease):
        raise ValueError('Invalid restore identity.')
    if destination not in (None, APPLE_WORKSPACE):
        raise ValueError('Invalid declared workspace.')
    workspace = Path(destination) if destination else Path.home()/'workspace'
    if not workspace.parent.is_dir() or workspace.parent.is_symlink():
        raise ValueError('The declared workspace parent is unavailable.')
    marker = '.lease-restore-receipt'
    identity = {'sha256': digest, 'lease': lease}
    os.umask(0o077)
    with tempfile.TemporaryDirectory(dir=workspace.parent) as tmp:
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
            extract_workspace(source, target)
        with (target / marker).open('w') as receipt:
            json.dump(identity, receipt)
        # A nonempty destination cannot be replaced by a directory rename.
        target.rename(workspace)
    return 'restored'


if __name__ == '__main__':
    try:
        if len(sys.argv) == 3 and sys.argv[1] == '--validate':
            if sys.platform.startswith('linux'):
                resource.setrlimit(resource.RLIMIT_AS, (192 * 1024**2, 192 * 1024**2))
                resource.setrlimit(resource.RLIMIT_CPU, (60, 60))
            extract_workspace(sys.stdin.buffer, Path(sys.argv[2]))
        else:
            print(restore(*sys.argv[1:]))
    except (OSError, ValueError, tarfile.TarError) as error:
        sys.exit(f'Workspace restore failed: {error}')
