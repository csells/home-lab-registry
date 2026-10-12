import unittest,time
from model import inventory
class Separation(unittest.TestCase):
 def record(self,id,kind=None,cap=None):
  return {'id':id,'spec':{'name':id,'host':'test','capabilities':cap or [],'endpoints':{},'lifecycle':'active','resources':{'kind':kind}},'health':{'status':'healthy','checked':1,'expires':2}}
 def test_machines_never_services_and_archives_excluded(self):
  rows=[self.record('machine', 'machine'),self.record('history','historical-evidence'),self.record('app')];out=inventory(rows,{})
  self.assertEqual([r['id'] for r in out['machines']],['machine']);self.assertEqual([r['id'] for r in out['services']],['app']);self.assertEqual(out['services'][0]['machine_id'],'machine')
 def test_missing_readings_not_zero_or_healthy(self):
  out=inventory([self.record('m','machine')],{});self.assertEqual(out['machines'][0]['metrics'],{'status':'unconfigured'})
 def test_metrics_do_not_replace_reachability(self):
  out=inventory([self.record('m','machine')],{'m':{'status':'unavailable'}});self.assertEqual(out['machines'][0]['health']['status'],'healthy');self.assertEqual(out['machines'][0]['metrics']['status'],'unavailable')
 def test_private_metadata_not_exposed(self):
  r=self.record('m','machine');r['spec']['resources']['access']={'password':'SECRET'};self.assertNotIn('SECRET',str(inventory([r],{})))
 def test_backup_job_is_not_a_service(self):
  r=self.record('job','backup-job');out=inventory([r],{})
  self.assertEqual(out['services'],[]);self.assertEqual([x['id'] for x in out['backups']],['job'])
 def test_virtual_machine_is_not_a_service(self):
  r=self.record('vm','virtual-machine');out=inventory([r],{})
  self.assertEqual(out['services'],[]);self.assertEqual([x['id'] for x in out['machines']],['vm'])
 def test_stopped_is_explicit_and_retired_is_excluded(self):
  r=self.record('vm','machine');r['spec']['resources']['runtime']={'state':'stopped','checked':123}
  retired=self.record('old');retired['spec']['lifecycle']='retired'
  out=inventory([r,retired],{})
  self.assertEqual(out['machines'][0]['runtime'],{'state':'stopped','checked':123})
  self.assertEqual(out['services'],[])
 def test_services_are_not_assigned_to_arbitrary_guest_on_same_host(self):
  physical=self.record('physical','machine');guest=self.record('guest','virtual-machine');app=self.record('app')
  self.assertEqual(inventory([physical,guest,app],{})['services'][0]['machine_id'],'physical')
if __name__=='__main__':unittest.main(verbosity=2)
