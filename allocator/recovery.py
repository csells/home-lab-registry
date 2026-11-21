"""Serve a checkpoint after redeeming an owner's short-lived recovery ticket."""
import argparse
import json
import re
import shutil
import ssl
import tempfile
import threading
import time
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from checkpoints import MAX_ARCHIVE_BYTES
from client import request
from exporter import ExportError, collect, digest, store


class RecoveryServer(ThreadingHTTPServer):
    def __init__(self, address, handler):
        self.capacity = threading.BoundedSemaphore(2)
        super().__init__(address, handler)

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(8)
        return connection, address

    def process_request(self, connection, address):
        if not self.capacity.acquire(blocking=False):
            self.shutdown_request(connection)
            return
        try:
            super().process_request(connection, address)
        except BaseException:
            self.capacity.release()
            raise

    def process_request_thread(self, connection, address):
        try:
            super().process_request_thread(connection, address)
        finally:
            self.capacity.release()


def controller(config, route, data):
    return request(config['controller_endpoint'], Path(config['ca_file']),
                   Path(config['worker_token_file']), route, data)


def retrieve(config, ticket, archive):
    proof = controller(config, '/v1/redeem-checkpoint', {'ticket': ticket})
    snapshot, path, checksum = (proof[key] for key in ('snapshot', 'path', 'sha256'))
    if (not re.fullmatch(r'[a-f0-9]{64}', snapshot) or not re.fullmatch(r'/lease-[a-f0-9]{32}(?:-[a-f0-9]{32})?\.tar\.gz', path)
            or not re.fullmatch(r'[a-f0-9]{64}', checksum)):
        raise ExportError('Unexpected checkpoint identity')
    collect(config['restic_command'] + ['dump', snapshot, path], archive,
            config.get('max_archive_bytes', 64 * 1024 ** 2), 120)
    if digest(archive) != checksum:
        raise ExportError('Checkpoint stored digest did not match')
    archive.seek(0, 2)
    size = archive.tell()
    archive.seek(0)
    return size


def receive(config, grant, size, incoming, archive):
    if (size != grant['size']
            or not 0 < size <= min(MAX_ARCHIVE_BYTES, config.get('max_archive_bytes', MAX_ARCHIVE_BYTES))
            or not re.fullmatch(r'/lease-[a-f0-9]{32}-[a-f0-9]{32}\.tar\.gz', grant['path'])
            or not grant['path'].endswith('-' + grant['id'] + '.tar.gz')):
        raise ExportError('Archive identity or size does not match the grant.')
    deadline = time.monotonic() + 60
    while size:
        if time.monotonic() >= deadline:
            raise ExportError('Upload exceeded its receive deadline.')
        chunk = incoming.read1(min(65536, size))
        if not chunk:
            raise ExportError('Archive upload ended early.')
        archive.write(chunk)
        size -= len(chunk)
    archive.seek(0)
    if digest(archive) != grant['sha256']:
        raise ExportError('Uploaded archive does not match its declared digest.')
    return store(config, archive, grant['path'][1:], grant['id'], 65)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    config = json.loads(parser.parse_args().config.read_text())

    class Handler(BaseHTTPRequestHandler):
        def json_response(self, status, value):
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def json_error(self, status, code):
            self.json_response(status, {'code': code})

        def do_POST(self):
            if self.path != '/v1/checkpoint' or config.get('accept_uploads') is not True:
                self.json_error(404, 'unknown_route')
                return
            header = self.headers.get('Authorization', '')
            if not re.fullmatch(r'Bearer [a-zA-Z0-9_-]{43}', header):
                self.json_error(401, 'invalid_ticket')
                return
            ticket = header.removeprefix('Bearer ')
            try:
                size = int(self.headers.get('Content-Length', '0'))
                grant = controller(config, '/v1/redeem-upload', {'ticket': ticket})
            except ValueError:
                self.json_error(400, 'invalid_length')
                return
            except urllib.error.HTTPError as error:
                error.close()
                self.json_error(error.code, 'upload_grant_unavailable')
                return
            except OSError:
                self.json_error(502, 'upload_grant_unavailable')
                return
            try:
                try:
                    with tempfile.TemporaryFile() as archive:
                        receipt = receive(config, grant, size, self.rfile, archive)
                except (OSError, KeyError, ValueError, ExportError) as error:
                    reason = str(error) if isinstance(error, ExportError) else type(error).__name__
                    controller(config, '/v1/complete-upload', {'ticket': ticket, 'error': reason[:240]})
                    self.json_error(502, 'checkpoint_not_acknowledged')
                    return
                result = controller(config, '/v1/complete-upload', {'ticket': ticket, **receipt})
            except OSError:
                self.json_error(502, 'checkpoint_acknowledgment_unavailable')
                return
            self.json_response(200, result)

        def do_GET(self):
            if self.path != '/v1/checkpoint':
                self.json_error(404, 'unknown_route')
                return
            header = self.headers.get('Authorization', '')
            if not re.fullmatch(r'Bearer [a-zA-Z0-9_-]{43}', header):
                self.json_error(401, 'invalid_ticket')
                return
            with tempfile.TemporaryFile() as archive:
                try:
                    size = retrieve(config, header.removeprefix('Bearer '), archive)
                except urllib.error.HTTPError as error:
                    error.close()
                    if error.code == 404:
                        self.json_error(404, 'ticket_unavailable')
                    else:
                        self.json_error(502, 'recovery_unavailable')
                    return
                except (OSError, ValueError, KeyError, ExportError) as error:
                    print(f'Checkpoint retrieval failed: {type(error).__name__}', flush=True)
                    self.json_error(502, 'checkpoint_unavailable')
                    return
                self.send_response(200)
                self.send_header('Content-Type', 'application/gzip')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(size))
                self.end_headers()
                shutil.copyfileobj(archive, self.wfile, 65536)

    with RecoveryServer((config['listen'], int(config['port'])), Handler) as server:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_3
        context.load_cert_chain(config['tls']['cert_file'], config['tls']['key_file'])
        server.socket = context.wrap_socket(server.socket, server_side=True, do_handshake_on_connect=False)
        print('Checkpoint recovery listening with TLS', flush=True)
        server.serve_forever()


if __name__ == '__main__':
    main()
