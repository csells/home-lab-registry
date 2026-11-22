import io
import unittest
from email.message import Message

from catalog_updates import consume


class Stream(io.BytesIO):
    headers = Message()
    headers['Content-Type'] = 'application/x-ndjson'


class CatalogUpdatesTests(unittest.TestCase):
    def test_changes_invalidate_once_and_reconnect_reloads(self):
        seen = []

        def invalidate():
            seen.append('reload')

        with self.assertRaises(ConnectionError):
            consume(Stream(b'{"version":1}\n{"heartbeat":true}\n{"version":1}\n{"version":4}\n'),
                    invalidate)
        self.assertEqual(len(seen), 2)
        with self.assertRaises(ConnectionError):
            consume(Stream(b'{"version":4}\n'), invalidate)
        self.assertEqual(len(seen), 3)

    def test_truncated_frame_or_wrong_version_is_rejected(self):
        for data in (b'{"version": 1}', b'{"version": true}\n', b'x' * 257):
            with self.subTest(data=data), self.assertRaises(ValueError):
                consume(Stream(data), lambda: self.fail('Invalid event triggered reload'))
