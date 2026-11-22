import io
import unittest
from email.message import Message

from catalog_updates import consume


class Stream(io.BytesIO):
    headers = Message()
    headers['Content-Type'] = 'application/x-ndjson'


class CatalogUpdatesTests(unittest.TestCase):
    def test_current_cursor_is_ignored_until_a_change_or_restart(self):
        seen = []
        epoch = '1' * 32
        later_epoch = '2' * 32
        frames = (f'{{"epoch":"{epoch}","version":1}}\n{{"heartbeat":true}}\n'
                  f'{{"epoch":"{epoch}","version":4}}\n').encode()
        with self.assertRaises(ConnectionError):
            consume(Stream(frames), seen.append, (epoch, 1))
        self.assertEqual(seen, [(epoch, 4)])
        with self.assertRaises(ConnectionError):
            consume(Stream(f'{{"epoch":"{later_epoch}","version":4}}\n'.encode()),
                    seen.append, (epoch, 4))
        self.assertEqual(seen, [(epoch, 4), (later_epoch, 4)])

    def test_older_stream_without_epoch_is_accepted(self):
        seen = []
        with self.assertRaises(ConnectionError):
            consume(Stream(b'{"version":1}\n{"version":1}\n'), seen.append)
        self.assertEqual(seen, [(None, 1)])

    def test_truncated_frame_or_wrong_version_is_rejected(self):
        for data in (b'{"version": 1}', b'{"version": true}\n',
                     b'{"version": -1}\n', b'{"version": 1, "epoch": 12}\n', b'x' * 257):
            with self.subTest(data=data), self.assertRaises(ValueError):
                consume(Stream(data), lambda cursor: self.fail('Invalid event triggered reload'))
