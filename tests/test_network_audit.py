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
  a=NetworkAudit('/missing');a._run();self.assertEqual(a.snapshot()['status'],'error')
if __name__=='__main__':unittest.main()

class CollectorTests(unittest.TestCase):
 def test_mac_from_link_inventory_not_ipv4_addr(self):
  import network_collect
  with patch('network_collect.run',side_effect=['[{"ifname":"eth0","addr_info":[{"family":"inet","local":"192.0.2.10","dynamic":true}]}]','[{"ifname":"eth0","address":"aa:bb:cc:dd:ee:ff"}]']),patch('network_collect.glob.glob',return_value=[]),patch('network_collect.os.path.isfile',return_value=False):
   rows=network_collect.linux()
  self.assertEqual(rows[0]['mac'],'aa:bb:cc:dd:ee:ff')
