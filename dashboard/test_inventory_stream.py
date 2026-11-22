import email.message,io,json,threading,time,unittest
from unittest.mock import patch
from urllib.error import HTTPError
from machine_runtime import InventorySubscriptions,parse_report

class InventoryStreamTests(unittest.TestCase):
 def test_report_preserves_complete_entries_and_rejects_duplicate(self):
  now=time.time();g={'id':'1','state':'running','name':'guest','type':'qemu'}
  value={'status':'available','checked':now,'expires':now+300,'guests':[g]}
  self.assertEqual(parse_report(value)['entries'],[g])
  value['guests']=[g,g]
  with self.assertRaises(ValueError):parse_report(value)
 def test_hardware_evidence_has_independent_freshness(self):
  now=time.time()
  reading={'os':'linux','architecture':'amd64','logical_cpus':4,'memory_bytes':16<<30,'checked':now}
  value={'status':'unavailable','warnings':['provider offline'],'hardware':reading}
  self.assertEqual(parse_report(value)['hardware'],reading)
  self.assertIsNone(parse_report({**value,'hardware':{**reading,'checked':now+60}})['hardware'])
  self.assertIsNone(parse_report({**value,'hardware':{**reading,'logical_cpus':True}})['hardware'])
 def test_configure_does_not_poll_an_unchanged_endpoint(self):
  monitor=InventorySubscriptions(None,threading.Event());endpoints={'host':('https://host:9911/stream','host')}
  with patch('machine_runtime.threading.Thread') as thread:
   for _ in range(10):monitor.configure(endpoints)
  self.assertEqual(thread.call_count,1)
 def test_old_connection_cannot_overwrite_new_snapshot(self):
  monitor=InventorySubscriptions(None,threading.Event());stop=threading.Event();stop.set()
  monitor._publish('host',stop,{'error':'old'})
  self.assertEqual(monitor.snapshot(),{})
 def test_unsupported_stream_waits_for_reconfiguration_not_snapshot_poll(self):
  monitor=InventorySubscriptions(None,threading.Event());stop=threading.Event()
  e=HTTPError('https://host/guests/stream',404,'missing',email.message.Message(),None)
  with patch('machine_runtime.connect',side_effect=e) as connect:
   t=threading.Thread(target=monitor._stream,args=('host',('https://host:9911/stream','host'),stop));t.start()
   self.assertTrue(monitor.changed.wait(1));time.sleep(.05);stop.set();t.join(1)
  self.assertEqual(connect.call_count,1)
  self.assertIn('/guests/stream',connect.call_args.args[0])
