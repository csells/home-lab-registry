"""Synthetic-catalog review UI. Loopback only; expose through an authenticated relay."""
import pathlib,sys,threading
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parents[1]/'dashboard'))
import server
server.LIVE=server.LiveMetrics(server.PRIVATE)
s=server.ThreadingHTTPServer(('127.0.0.1',8089),server.Handler)
s.auth=''  # Synthetic test data only; outer BB relay requires owner's login.
threading.Thread(target=server.reconcile,daemon=True).start()
s.serve_forever()
