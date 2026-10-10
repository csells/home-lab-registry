#!/usr/bin/env python3
"""Loopback-only read-only dashboard behind an authenticated reverse proxy."""
import argparse, json, logging, pathlib, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from client import request

def public_record(record):
    spec=record['spec']
    backup=spec.get('config_backup',{'status':'unknown','locations':[]})
    backup={k:backup[k] for k in ('status','locations','notes') if k in backup}
    backup['locations']=[{k:loc[k] for k in ('host','path')} for loc in backup.get('locations',[])]
    return dict(config_backup=backup,id=record['id'],name=spec['name'],host=spec['host'],capabilities=spec['capabilities'],endpoints=spec['endpoints'],lifecycle=spec['lifecycle'],health={k:v for k,v in record['health'].items() if k in ('status','evidence','checked','expires')})
