import email.message,io,json,pathlib,time,unittest,urllib.error
from unittest.mock import patch
import machine_runtime
from machine_runtime import RuntimeMonitor,overlay,read_guests
from model import address_audit_summary

BASE=pathlib.Path(__file__).parent

class Response(io.BytesIO):
 def __init__(self,value,kind='application/json'):
  super().__init__(json.dumps(value).encode());self.headers=email.message.Message();self.headers['Content-Type']=kind
 def __enter__(self):return self
 def __exit__(self,*args):self.close()

def report(status='available',guests=None,checked=None,ttl=90,warnings=None):
 checked=time.time() if checked is None else checked
 return {'status':status,'source':'proxmox','checked':checked,'expires':checked+ttl,'guests':guests,'warnings':warnings or []}

def vm(gid=101):return {'id':'vm','spec':{'host':'pve','capabilities':['machine'],'resources':{'kind':'virtual-machine','parent_machine':'machine-pve','guest_id':gid}}}

class GuestRuntimeTests(unittest.TestCase):
 def setUp(self):
  # The runtime path must never shell out: SSH is gone.
  self.no_exec=patch('subprocess.run',side_effect=AssertionError('subprocess used'))
  self.no_exec.start();self.addCleanup(self.no_exec.stop)

 def test_runtime_module_has_no_ssh_path(self):
  self.assertFalse(hasattr(machine_runtime,'subprocess'))
  self.assertNotIn('ssh',(BASE/'machine_runtime.py').read_text().lower())

 def test_state_comes_from_the_authenticated_agent_endpoint(self):
  value=report(guests=[{'id':'101','name':'web','type':'qemu','state':'running'}])
  with patch('machine_runtime.connect',return_value=Response(value)) as connect:
   monitor=RuntimeMonitor(pathlib.Path('/private'));monitor._read({'machine-pve':('https://192.0.2.5:9911/stream','pve')})
  url,private,identity=connect.call_args.args[:3]
  self.assertEqual((url,identity),('https://192.0.2.5:9911/guests','pve'))
  self.assertEqual(connect.call_args.kwargs.get('accept'),'application/json')
  runtime=monitor.apply([vm()])[0]['spec']['resources']['runtime']
  self.assertEqual(runtime['state'],'running');self.assertEqual(runtime['expires'],value['expires'])

 def test_agent_warning_is_unknown_not_stopped(self):
  value=report(status='unavailable',warnings=['Guest inventory unavailable: token lacks VM.Audit on /vms'])
  with patch('machine_runtime.connect',return_value=Response(value)):state=read_guests('https://192.0.2.5:9911/guests',None,None)
  self.assertIn('VM.Audit',state['error'])
  self.assertEqual(overlay([vm()],{'machine-pve':state})[0]['spec']['resources']['runtime']['state'],'unknown')

 def test_expired_report_is_unknown(self):
  old=time.time()-1000
  value=report(guests=[{'id':'101','name':'web','type':'qemu','state':'running'}],checked=old)
  with patch('machine_runtime.connect',return_value=Response(value)):state=read_guests('https://192.0.2.5:9911/guests',None,None)
  self.assertEqual(overlay([vm()],{'machine-pve':state})[0]['spec']['resources']['runtime']['state'],'unknown')

 def test_agent_expiry_cannot_extend_beyond_fifteen_minutes(self):
  now=time.time();state={'checked':now-1000,'expires':now+1000,'guests':{'101':'running'}}
  self.assertEqual(overlay([vm()],{'machine-pve':state},now=now)[0]['spec']['resources']['runtime']['state'],'unknown')

 def test_missing_guest_and_unconfigured_agent_are_unknown(self):
  err=urllib.error.HTTPError('https://192.0.2.5:9911/guests',404,'Not Found',email.message.Message(),io.BytesIO(b''))
  with patch('machine_runtime.connect',side_effect=err),self.assertLogs(level='WARNING'):state=read_guests('https://192.0.2.5:9911/guests',None,None)
  self.assertEqual(state['error'],'Guest inventory not configured on agent')
  fresh={'checked':time.time(),'expires':time.time()+90,'guests':{'999':'running'}}
  for s in (state,fresh):self.assertEqual(overlay([vm()],{'machine-pve':s})[0]['spec']['resources']['runtime']['state'],'unknown')

 def test_malformed_reports_are_rejected(self):
  for value,kind in [({'status':'available','guests':'x','checked':1,'expires':2},'application/json'),({'status':'available','guests':[{'id':1,'state':'running'}],'checked':1,'expires':2},'application/json'),({'status':'bogus'},'application/json'),(report(guests=[]),'text/html')]:
   with patch('machine_runtime.connect',return_value=Response(value,kind)),self.assertLogs(level='WARNING'):
    self.assertIn('error',read_guests('https://192.0.2.5:9911/guests',None,None))

 def test_refresh_is_single_flight_and_bounded(self):
  monitor=RuntimeMonitor(pathlib.Path('/private'))
  with patch('machine_runtime.threading.Thread') as worker:
   monitor.refresh({'a':('https://192.0.2.5:9911/stream',None)});monitor.refresh({'a':('https://192.0.2.5:9911/stream',None)})
  self.assertEqual(worker.call_count,1)

class NetworkAuditRemovalTests(unittest.TestCase):
 def test_dashboard_cannot_start_an_audit(self):
  self.assertNotIn('network_audit',(BASE/'server.py').read_text())
  self.assertNotIn('network-check',(BASE/'index.html').read_text())
  self.assertNotIn('api/network-audit',(BASE/'app.js').read_text())

 def test_latest_published_audit_is_summarized_read_only(self):
  records=[{'id':'router','spec':{'resources':{'fleet_address_audit':{'latest_run':{'at':'2026-10-19T13:20:49Z','status':'warning','warnings':[{'kind':'offline-config-unverified','target':'x'}],'verified_unreserved_static_count':0,'evidence_ref':'/private/path'}}}}}]
  self.assertEqual(address_audit_summary(records),{'status':'warning','at':'2026-10-19T13:20:49Z','warnings':1,'unreserved_static':0,'record':'router'})
  self.assertIsNone(address_audit_summary([]))

if __name__=='__main__':unittest.main()
