"""Archive a quiesced, task-owned workspace without following links."""
import argparse
import os
import stat
import sys
import tarfile
from pathlib import Path

from restore_workspace import MAX_ARCHIVE, MAX_EXPANDED, MAX_MEMBERS


class BoundedWriter:
    def __init__(self, stream):
        self.stream = stream
        self.written = 0

    def write(self, data):
        self.written += len(data)
        if self.written > MAX_ARCHIVE:
            raise ValueError('Compressed workspace exceeds the checkpoint budget')
        return self.stream.write(data)

    def flush(self):
        self.stream.flush()


def archive_workspace(workspace, output):
    if not workspace.is_absolute() or workspace.resolve(strict=True) != workspace or not workspace.is_dir():
        raise ValueError('Use the exact absolute task workspace')
    entries = 0
    byte_count = 0
    root_device = workspace.stat().st_dev
    with tarfile.open(fileobj=BoundedWriter(output), mode='w|gz', dereference=False) as archive:
        for parent, dirs, files in os.walk(workspace, followlinks=False, onerror=_raise):
            dirs.sort()
            for name in [*dirs, *sorted(files)]:
                path = Path(parent) / name
                info = path.lstat()
                rel = path.relative_to(workspace)
                entries += 1
                if (entries > MAX_MEMBERS or info.st_dev != root_device or any(
                        part in ('.', '..') for part in rel.parts)):
                    raise ValueError('Workspace exceeds its entry budget or crosses a filesystem')
                if stat.S_ISLNK(info.st_mode):
                    destination = Path(os.readlink(path))
                    if destination.is_absolute() or not (path.parent / destination).resolve().is_relative_to(workspace):
                        raise ValueError('Workspace link points outside the workspace')
                elif stat.S_ISREG(info.st_mode):
                    byte_count += info.st_size
                    if info.st_nlink > 1 or byte_count > MAX_EXPANDED:
                        raise ValueError('Workspace contains a hard link or exceeds the byte budget')
                elif not stat.S_ISDIR(info.st_mode):
                    raise ValueError('Workspace contains an unsupported entry')
                archive.add(path, arcname=str(rel), recursive=False)


def _raise(error):
    raise error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('workspace', type=Path)
    args = parser.parse_args()
    try:
        archive_workspace(args.workspace, sys.stdout.buffer)
    except (OSError, RuntimeError, tarfile.TarError, ValueError) as error:
        print(f'Workspace archive failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
