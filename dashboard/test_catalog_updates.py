import io
import unittest
from threading import Event
from urllib.error import HTTPError
from unittest.mock import patch
from email.message import Message

from catalog_updates import consume, subscribe_catalog


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

    def test_rejected_feed_reports_unverified_and_recovers(self):
        stop = Event()
        states = []
        expired = HTTPError('https://registry.invalid', 401, 'Unauthorized', {}, io.BytesIO())
        valid = Stream(b'{"version":1}\n')
        def status(value):
            states.append(value)
            if value:
                stop.set()
        with patch('catalog_updates.ssl.create_default_context'), patch('catalog_updates.build_opener') as build:
            build.return_value.open.side_effect = (expired, valid)
            subscribe_catalog({'url':'https://registry.invalid','ca_file':'ca','reader_token':'reader'},
                              lambda: None, status, stop)
        self.assertEqual(states[:2], [False, True])

    def test_truncated_frame_or_wrong_version_is_rejected(self):
        for data in (b'{"version": 1}', b'{"version": true}\n',
                     b'{"version": -1}\n', b'{"version": 1, "epoch": 12}\n', b'x' * 257):
            with self.subTest(data=data), self.assertRaises(ValueError):
                consume(Stream(data), lambda cursor: self.fail('Invalid event triggered reload'))
