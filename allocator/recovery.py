"""Serve a checkpoint after redeeming an owner's short-lived recovery ticket."""
import argparse
import json
import re
import shutil
import ssl
import tempfile
import threading
import urllib.error
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from client import request
from exporter import ExportError, collect, digest


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


def retrieve(config, ticket, archive):
    proof = request(config['controller_endpoint'], Path(config['ca_file']),
                    Path(config['worker_token_file']), '/v1/redeem-checkpoint', {'ticket': ticket})
    snapshot, path, checksum = (proof[key] for key in ('snapshot', 'path', 'sha256'))
    if (not re.fullmatch(r'[a-f0-9]{64}', snapshot) or not re.fullmatch(r'/lease-[a-f0-9]{32}\.tar\.gz', path)
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    config = json.loads(parser.parse_args().config.read_text())

    class Handler(BaseHTTPRequestHandler):
        def json_error(self, status, code):
            body = json.dumps({'code': code}).encode()
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

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
