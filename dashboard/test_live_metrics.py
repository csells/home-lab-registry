import unittest,json,tempfile,pathlib,time,threading,email.message
from unittest.mock import patch
from metrics import LiveMetrics,Stream

class Response:
 def __init__(self):
  self.headers=email.message.Message();self.headers['Content-Type']='application/x-ndjson';self.first=True;self.closed=False
 def __enter__(self):return self
 def __exit__(self,*args):self.closed=True
 def readline(self,n):
  if self.first:
   self.first=False;return json.dumps({'type':'history','history':[{'checked':time.time(),'expires':time.time()+30,'status':'available'}]}).encode()+b'\n'
  time.sleep(.05);return b'{"type":"heartbeat"}\n'

class LiveMetricsTest(unittest.TestCase):
 def test_history_bounded_and_failure_retains_history(self):
  s=Stream('',None)
  for n in range(130):s.receive({'type':'samples','samples':[{'checked':n,'status':'available','expires':9999999999}]})
  self.assertEqual(len(s.history),120);self.assertEqual(s.history[0]['checked'],10)
  s.error='Disconnected';r=s.reading();self.assertEqual(r['status'],'unavailable');self.assertEqual(len(r['history']),120)
  for frame in [None,{}, {'type':'samples','samples':[{}]}]:
   with self.assertRaises(ValueError):s.receive(frame)
 def test_two_viewers_share_connection_and_idle_closes_it(self):
  with tempfile.TemporaryDirectory() as d:
   live=LiveMetrics(pathlib.Path(d));live.targets={'machine-test':'https://example.test:9911/stream'};response=Response()
   with patch('metrics.connect',return_value=response) as connect:
    with live.subscribe('machine-test') as first:
     with live.subscribe('machine-test') as second:
      self.assertIs(first,second)
      deadline=time.monotonic()+5
      with first.cv:
       while not first.history and time.monotonic()<deadline:first.cv.wait(.1)
       self.assertEqual(len(first.history),1)
      self.assertEqual(first.viewers,2);self.assertEqual(connect.call_count,1)
     self.assertFalse(response.closed);self.assertEqual(first.viewers,1)
    first.worker.join(2);self.assertFalse(first.worker.is_alive());self.assertTrue(response.closed);self.assertFalse(live.streams)
   with self.assertRaises(KeyError):
    with live.subscribe('arbitrary-host'):pass
 def test_registry_is_discovery_source_and_only_exact_https_streams(self):
  with tempfile.TemporaryDirectory() as d:
   live=LiveMetrics(pathlib.Path(d));record={'id':'machine-node','spec':{'lifecycle':'active','resources':{'metrics_stream':{'enabled':True,'url':'https://192.0.2.10:9911/stream'}}}}
   live.configure([record]);self.assertIn('machine-node',live.targets)
   for url in ['http://192.0.2.10:9911/stream','https://user:pass@192.0.2.10:9911/stream','https://192.0.2.10:9911/secrets','https://192.0.2.10:9911/stream?key=value']:
    record['spec']['resources']['metrics_stream']['url']=url;live.configure([record]);self.assertFalse(live.targets)
if __name__=='__main__':unittest.main()
