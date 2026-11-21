"""Loopback-only review of allowlisted, redacted collector evidence."""
import argparse
import html
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path not in ('/', '/report.json', '/healthz'):
            self.send_error(404)
            return
        if self.path == '/healthz':
            body=b'ok';content_type='text/plain'
        else:
            report=json.loads(self.server.report.read_text())
            if self.path == '/report.json':
                body=json.dumps(report,indent=2).encode();content_type='application/json'
            else:
                rows=''.join('<tr><td>'+html.escape(r['source'])+'</td><td>'+html.escape(r['result'])+'</td><td>'+html.escape(r['recovery'])+'</td></tr>' for r in report['sources'])
                tests=''.join('<li>'+html.escape(t)+'</li>' for t in report['checks'])
                gaps=''.join('<li>'+html.escape(t)+'</li>' for t in report['remaining'])
                body=('''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Backup collector staging</title><style>body{font:17px/1.6 system-ui,sans-serif;color:#19332d;background:#f4f6f2;margin:0}main{max-width:960px;margin:auto;padding:40px 24px}h1{font-size:40px;line-height:1.15}h2{margin-top:36px}table{width:100%;border-collapse:collapse;background:white}td,th{text-align:left;padding:16px;border-bottom:1px solid #ccd6cf}.notice{background:#fff0c5;padding:18px;border-left:5px solid #b77b00}a{color:#00664b}button{font:inherit;background:#145942;color:white;border:0;border-radius:5px;padding:10px 20px;cursor:pointer}details{background:white;padding:16px;margin:16px 0}#updated{font-size:14px;color:#52665e}</style><main><p>HOME LAB · STAGING REVIEW</p><h1>Backups reach the right machine.<br>Recovery has evidence.</h1><p id="updated">'''+html.escape(report['checked_at'])+'''</p><p class="notice">Review qualification results. Production is unchanged; full fleet migration is not ready for approval.</p><h2>What was recovered</h2><table><thead><tr><th>Source</th><th>Collection</th><th>Recovery check</th></tr></thead><tbody>'''+rows+'''</tbody></table><details><summary>Inspect safety and identity checks</summary><ul>'''+tests+'''</ul></details><h2>Still to qualify</h2><ul>'''+gaps+'''</ul><p><button id="refresh" onclick="location.reload()">Refresh results</button> <a href="/report.json">Download redacted evidence</a></p><p>Archives, credentials, private addresses and file contents are not served by this page. Tests do not run when you refresh.</p></main></html>''').encode();content_type='text/html; charset=utf-8'
        self.send_response(200);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(body)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.send_header('Content-Security-Policy',"default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'");self.end_headers();self.wfile.write(body)


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--report',type=Path,required=True);p.add_argument('--port',type=int,default=9084);a=p.parse_args()
    server=ThreadingHTTPServer(('127.0.0.1',a.port),Handler);server.report=a.report;server.serve_forever()
