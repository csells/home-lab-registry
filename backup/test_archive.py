import hashlib
import gzip
import io
import os
import pathlib
import tarfile
import tempfile
import unittest

from collect import verify_archive


class ArchiveCompatibility(unittest.TestCase):
    def test_mac_pax_headers_validate_without_changing_native_restore_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory)/'config.tar.gz'
            with tarfile.open(path, 'w:gz', format=tarfile.PAX_FORMAT) as archive:
                member = tarfile.TarInfo('config.json')
                member.pax_headers = {'LIBARCHIVE.xattr.com.apple.provenance': 'opaque'}
                member.size = 2
                archive.addfile(member, io.BytesIO(b'{}'))
            before = hashlib.sha256(path.read_bytes()).digest()
            verify_archive(path)
            self.assertEqual(before, hashlib.sha256(path.read_bytes()).digest())

    def test_invalid_and_truncated_archives_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory)/'config.tar.gz'
            path.write_bytes(b'not an archive'*100)
            with self.assertRaises(gzip.BadGzipFile):
                verify_archive(path)
            with tarfile.open(path, 'w:gz') as archive:
                member = tarfile.TarInfo('config')
                member.size = 4096
                archive.addfile(member, io.BytesIO(os.urandom(4096)))
            path.write_bytes(path.read_bytes()[:-5])
            with self.assertRaises(EOFError):
                verify_archive(path)


if __name__ == '__main__':
    unittest.main()
