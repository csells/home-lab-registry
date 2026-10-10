"""Registry-configured connectivity checks; no device commands are sent."""
import socket

def check(probe):
    try:
        with socket.create_connection((probe['tcp_host'],probe['tcp_port']),timeout=3):
            pass
        return True, 'TCP reachable (connectivity only; device operations not tested)'
    except OSError as error:
        return False, 'TCP unavailable: '+type(error).__name__
