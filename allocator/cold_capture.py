"""Stream the declared workspace from an immutable raw Linux slot image."""
import argparse
import gzip
import os
import shutil
import subprocess
import tempfile
from contextlib import closing, contextmanager
import json
import posixpath
import re
import sys
from pathlib import Path


@contextmanager
def sealed_image(config):
    """Spool one provider-locked disk, and discard it after extraction."""
    expected = config['disk_bytes']
    budget = config['max_spool_bytes']
    if not (type(expected) is int and type(budget) is int and 0 < budget <= expected):
        raise ValueError('Invalid disk or temporary storage budget.')
    command = config['source_command']
    if not isinstance(command, list) or not command or not all(isinstance(arg, str) for arg in command):
        raise ValueError('Configure a fixed stopped-slot source command.')
    block = 1024 * 1024
    with tempfile.TemporaryDirectory(dir=config['spool_dir']) as spool:
        path = Path(spool) / 'slot.raw'
        if shutil.disk_usage(spool).free < budget:
            raise ValueError('Insufficient temporary space for the configured capture budget.')
        with path.open('xb') as image, subprocess.Popen(
                ['/usr/bin/timeout', '--kill-after=5s', '180', *command],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE) as source:
            try:
                written = 0
                with gzip.GzipFile(fileobj=source.stdout) as stream:
                    while chunk := stream.read(block):
                        if image.tell() + len(chunk) > expected:
                            raise ValueError('Source exceeded its registered disk size.')
                        if chunk.count(0) == len(chunk):
                            image.seek(len(chunk), os.SEEK_CUR)
                        else:
                            written += len(chunk)
                            if written > budget:
                                raise ValueError('Source exceeded its temporary storage budget.')
                            image.write(chunk)
                if source.wait(timeout=10) != 0 or image.tell() != expected:
                    raise ValueError('Stopped-slot source was incomplete.')
                image.truncate()
            finally:
                source.stdout.close()
                if source.poll() is None:
                    source.terminate()
        path.chmod(0o400)
        yield str(path)


def capture(config, guest, lease, output):
    if guest != str(config['guest_id']) or not re.fullmatch(r'[a-f0-9]{32}', lease):
        raise ValueError('Capture identity does not match the configured slot.')
    workspace = config['workspace']
    if (not workspace.startswith('/') or workspace.startswith('//') or workspace == '/'
            or posixpath.normpath(workspace) != workspace):
        raise ValueError('Configure an absolute workspace directory below the root.')
    import guestfs
    with sealed_image(config) as image, closing(guestfs.GuestFS(python_return_dict=True)) as appliance:
        appliance.set_backend('direct')
        appliance.set_network(False)
        appliance.set_memsize(config.get('appliance_memory_mib', 512))
        appliance.add_drive_opts(image, readonly=True, format='raw')
        appliance.launch()
        disk, = appliance.list_devices()
        root, = [part for part in appliance.list_partitions()
                 if appliance.part_get_gpt_type(disk, appliance.part_to_partnum(part)).lower()
                 == '4f68bce3-e8cd-4db1-96e7-fbcaf984b709']
        appliance.mount_vfs('ro,noload,nodev,nosuid,noexec', 'ext4', root, '/')
        marker = '/etc/ssh/lease_authorized_keys'
        if (appliance.realpath(marker) != marker or not appliance.is_file(marker)
                or appliance.filesize(marker) > 256 or appliance.statns(marker)['st_uid'] != 0):
            raise ValueError('Missing protected lease identity on the source disk.')
        key = appliance.read_file(marker).split()
        if len(key) != 3 or key[0] != b'ssh-ed25519' or key[2] != ('lease-' + lease).encode():
            raise ValueError('The source disk belongs to another lease.')
        if appliance.realpath(workspace) != workspace or not appliance.is_dir(workspace):
            raise ValueError('The declared workspace is missing or resolves outside its configured path.')
        appliance.tar_out_opts(workspace, output, compress='gzip', numericowner=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    parser.add_argument('guest')
    parser.add_argument('lease')
    args = parser.parse_args()
    try:
        capture(json.loads(args.config.read_text()), args.guest, args.lease, '/dev/stdout')
    except (OSError, EOFError, subprocess.SubprocessError, RuntimeError, TypeError, ValueError, KeyError) as error:
        print(f'Cold capture failed: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
