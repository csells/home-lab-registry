import pathlib,sys,unittest,threading
from unittest.mock import patch
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parents[1]/'dashboard'))
from network_audit import compare,NetworkAudit
class AddressAuditTests(unittest.TestCase):
 def check(self,row,res=None):return compare([row],res or [],['192.0.2.0/24'])
 def test_static_requires_exact_mac(self):
  row={'address':'192.0.2.10','mac':'aa:bb:cc:dd:ee:ff','assignment':'static'}
  self.assertEqual(self.check(row,[{'ip':row['address'],'mac':'11:22:33:44:55:66'}])[0]['status'],'violation')
  self.assertEqual(self.check(row,[{'ip':row['address'],'mac':row['mac'].upper()}])[0]['status'],'pass')
 def test_dhcp_does_not_need_reservation(self):self.assertEqual(self.check({'address':'192.0.2.10','assignment':'dhcp'})[0]['status'],'pass')
 def test_unknown_is_not_pass(self):self.assertEqual(self.check({'address':'192.0.2.10','assignment':'unknown'})[0]['status'],'unverified')
 def test_unknown_static_interface_is_not_pass(self):self.assertEqual(self.check({'address':'192.0.2.10','assignment':'static'})[0]['status'],'unverified')
 def test_nat_excluded(self):self.assertEqual(self.check({'address':'10.77.0.1','assignment':'static'}),[])
 def test_failed_collection_stays_visible(self):self.assertEqual(self.check({'machine':'offline','reason':'offline'})[0]['status'],'unverified')
 def test_idle_does_no_work_and_click_single_flight(self):
  with patch('network_audit.threading.Thread') as worker:
   audit=NetworkAudit('/missing');self.assertEqual(audit.snapshot()['status'],'idle');worker.assert_not_called()
   audit.start();audit.start();self.assertEqual(worker.call_count,1)
 def test_missing_config_is_error(self):
  a=NetworkAudit('/missing')
  with self.assertLogs(level='ERROR'):a._run()
  self.assertEqual(a.snapshot()['status'],'error')
if __name__=='__main__':unittest.main()

class CollectorTests(unittest.TestCase):
 def test_mac_from_link_inventory_not_ipv4_addr(self):
  import network_collect
  with patch('network_collect.run',side_effect=['[{"ifname":"eth0","addr_info":[{"family":"inet","local":"192.0.2.10","dynamic":true}]}]','[{"ifname":"eth0","address":"aa:bb:cc:dd:ee:ff"}]']),patch('network_collect.glob.glob',return_value=[]),patch('network_collect.os.path.isfile',return_value=False):
   rows=network_collect.linux()
  self.assertEqual(rows[0]['mac'],'aa:bb:cc:dd:ee:ff')

class CoverageTests(unittest.TestCase):
 def test_offline_dhcp_is_not_live_pass(self):
  row={'machine':'template','scope':'offline','assignment':'dhcp','mac':'aa:bb:cc:dd:ee:ff','profile_verified':True}
  self.assertEqual(compare([row],[],['192.0.2.0/24'])[0]['status'],'offline-config-verified')
 def test_unreadable_offline_not_pass(self):
  self.assertEqual(compare([{'scope':'offline','assignment':'dhcp'}],[],['192.0.2.0/24'])[0]['status'],'unverified')
 def test_guest_mapping_uses_registered_identity(self):
  from network_audit import guest_identity
  self.assertEqual(guest_identity({'host':'host','resources':{'network_authority_audit':{'guest_id':140,'host':'host'}}}),('machine-host','140'))
 def test_runtime_failure_invalidates_old_state(self):
  from machine_runtime import overlay
  result=overlay([{'id':'vm','spec':{'host':'a','capabilities':['machine'],'resources':{'parent_machine':'machine-a','guest_id':1,'runtime':{'state':'stopped','checked':1}}}}],{'machine-a':{'error':'unreachable','checked':10}},now=11)
  self.assertEqual(result[0]['spec']['resources']['runtime']['state'],'unknown')
 def test_runtime_does_not_use_stale_running(self):
  from machine_runtime import overlay
  r={'id':'vm','spec':{'host':'a','capabilities':['machine'],'resources':{'parent_machine':'machine-a','guest_id':1}}}
  result=overlay([r],{'machine-a':{'checked':1,'guests':{'1':'running'}}},now=9999)
  self.assertEqual(result[0]['spec']['resources']['runtime']['state'],'unknown')
 def test_glinet_dhcp_false_is_static(self):
  from appliance_network import glinet_rows
  r=glinet_rows({'ok':True,'result':{'success':True,'config':{'ip_address':'192.0.2.5','mac_address':'aa:bb:cc:dd:ee:ff','is_dhcp':False}}})
  self.assertEqual(r[0]['assignment'],'static')

class ServiceRuntimeTests(unittest.TestCase):
 def test_guest_power_does_not_override_application_health(self):
  from machine_runtime import overlay
  r={'id':'app','spec':{'host':'a','capabilities':['http'],'resources':{'guest_id':1,'parent_machine':'machine-a'}}}
  self.assertNotIn('runtime',overlay([r],{'machine-a':{'checked':10,'guests':{'1':'running'}}},now=11)[0]['spec']['resources'])

class SavedProfileTests(unittest.TestCase):
 def test_empty_disk_not_assumed_dhcp(self):
  import tempfile
  from network_collect import saved_profiles
  with tempfile.TemporaryDirectory() as root:
   with self.assertRaises(RuntimeError):saved_profiles(root,'aa:bb:cc:dd:ee:ff')
   self.assertEqual(saved_profiles(root,'aa:bb:cc:dd:ee:ff',allow_empty=True),[])
 def test_pending_alias_is_collected_even_with_dhcp(self):
  import tempfile
  from network_collect import saved_profiles
  with tempfile.TemporaryDirectory() as root:
   p=pathlib.Path(root)/'etc/network';p.mkdir(parents=True)
   (p/'interfaces').write_text('iface eth0 inet dhcp\n')
   (p/'interfaces.new').write_text('iface eth0 inet static\n address 192.0.2.66/24\n')
   rows=saved_profiles(root,'aa:bb:cc:dd:ee:ff')
   self.assertEqual({r['assignment'] for r in rows},{'dhcp','static'})
   self.assertEqual(compare(rows,[],['192.0.2.0/24'])[-1]['status'],'violation')
 def test_offline_nat_is_visible_but_not_a_lan_violation(self):
  r=compare([{'address':'10.77.0.30','assignment':'static','scope':'offline'}],[],['192.0.2.0/24'])
  self.assertEqual(r[0]['status'],'not-applicable')

class CertificatePinTests(unittest.TestCase):
 def test_changed_certificate_aborts_before_http(self):
  import hashlib,ssl
  from unittest.mock import Mock
  from appliance_network import PinnedHTTPSConnection
  connection=object.__new__(PinnedHTTPSConnection)
  connection.expected=hashlib.sha256(b'approved certificate').digest()
  connection.sock=Mock();connection.sock.getpeercert.return_value=b'different certificate'
  connection.close=Mock()
  with patch('http.client.HTTPSConnection.connect'):
   with self.assertRaises(ssl.SSLCertVerificationError):connection.connect()
  connection.close.assert_called_once()

class RemoteApplianceTests(unittest.TestCase):
 def test_credentials_travel_on_stdin_not_process_arguments(self):
  from appliance_network import collect_via_ssh
  from unittest.mock import Mock
  with patch('network_audit.ssh_command',return_value=['ssh','reader@host']),patch('subprocess.run',return_value=Mock(returncode=0,stdout=b'[{"machine":"host/amt","assignment":"dhcp"}]')) as run:
   rows=collect_via_ssh({'id':'host/amt','type':'amt'},pathlib.Path('/private'),{'password':'test-secret'},{'id':'gateway'})
  args=run.call_args
  self.assertNotIn('test-secret',' '.join(args.args[0]))
  self.assertIn(b'test-secret',args.kwargs['input'])
  self.assertEqual(rows[0]['vantage'],'gateway')
 def test_remote_failure_stays_unverified(self):
  from appliance_network import collect_via_ssh
  from unittest.mock import Mock
  with patch('network_audit.ssh_command',return_value=['ssh','host']),patch('subprocess.run',return_value=Mock(returncode=1)),self.assertLogs(level='ERROR'):
   rows=collect_via_ssh({'id':'host/amt','type':'amt'},pathlib.Path('/private'),{}, {'id':'gateway'})
  self.assertEqual(rows[0]['status'],'unverified')
