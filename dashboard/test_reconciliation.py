import unittest
from reconciliation import reconcile
from machine_runtime import overlay

class ReconciliationTests(unittest.TestCase):
 def test_unknown_collector_does_not_claim_guest_deleted(self):
  host={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True}}}}
  service={'id':'web','spec':{'host':'h','owner':'team','capabilities':['web'],'resources':{'guest_id':7},'lifecycle':'active'}}
  result=reconcile([host,service],{'machine-h':{'error':'offline','checked':100}},now=110)
  self.assertEqual(result['coverage']['verified'],0)
  self.assertEqual(result['coverage_status'],'unverified')
  self.assertEqual([f['category'] for f in result['findings']],['coverage'])
  self.assertNotIn('missing-guest',[i['code'] for i in result['findings']])
 def test_real_inventory_comparison_and_lease(self):
  host={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True}}}}
  service={'id':'web','spec':{'host':'h','owner':'team','capabilities':['web'],'resources':{'guest_id':7,'expires_at':'1970-01-01T00:01:00Z'},'lifecycle':'active'}}
  sample={'checked':100,'expires':300,'entries':[{'id':'8','name':'unexpected','state':'running','type':'qemu'}]}
  result=reconcile([host,service],{'machine-h':sample},now=110)
  self.assertEqual(result['coverage_status'],'verified')
  self.assertEqual({f['code']:f['category'] for f in result['findings']},{
   'missing-guest':'guest-mismatch','unregistered-guest':'guest-mismatch','expired-lease':'record-review'})
 def test_old_and_future_observations_are_not_truth(self):
  host={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True}}}}
  for checked in [1,2000]:
   result=reconcile([host],{'machine-h':{'checked':checked,'expires':99999,'entries':[]}},now=1000)
   self.assertEqual(result['coverage']['verified'],0)
 def test_many_services_on_one_guest_are_valid(self):
  host={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True}}}}
  services=[{'id':n,'spec':{'host':'h','owner':'team','capabilities':['web'],'lifecycle':'active','resources':{'guest_id':7}}} for n in ['web','api']]
  result=reconcile([host]+services,{'machine-h':{'checked':100,'expires':300,'entries':[{'id':'7','state':'running','type':'qemu'}]}},now=110)
  self.assertEqual(result['findings'],[])

class RetirementTests(unittest.TestCase):
 def test_retired_records_never_claim_a_guest(self):
  h={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True}}}}
  retired={'id':'old','spec':{'host':'h','owner':'team','lifecycle':'retired','resources':{'guest_id':7}}}
  result=reconcile([h,retired],{'machine-h':{'checked':100,'expires':300,'entries':[{'id':'7','name':'reused','state':'running','type':'qemu'}]}},now=110)
  self.assertEqual([f['code'] for f in result['findings']],['unregistered-guest'])
 def test_only_active_guest_claims_affect_live_comparison(self):
  host={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'metrics_stream':{'enabled':True}}}}
  guest={'id':'guest','spec':{'host':'h','owner':'team','lifecycle':'planned','resources':{
    'kind':'virtual-machine','runtime_binding':{'kind':'guest','machine_id':'machine-h','guest_id':'7'}}}}
  report={'checked':100,'expires':300,'entries':[]}
  self.assertEqual(reconcile([host,guest],{'machine-h':report},now=110)['findings'],[])
  report['entries']=[{'id':'7','state':'stopped','type':'qemu'}]
  self.assertEqual([f['code'] for f in reconcile([host,guest],{'machine-h':report},now=110)['findings']],['unregistered-guest'])

 def test_documented_guest_binding_also_drives_power_state(self):
  record={'id':'guest','spec':{'host':'h','lifecycle':'active','resources':{
    'kind':'virtual-machine','guest_id':'wrong','runtime_binding':{'kind':'guest','machine_id':'machine-h','guest_id':'7'}}}}
  report={'checked':100,'expires':300,'guests':{'7':'stopped'}}
  state=overlay([record],{'machine-h':report},now=110)[0]['spec']['resources'].get('runtime',{})
  self.assertEqual(state.get('state'),'stopped')

 def test_registered_template_does_not_need_a_service(self):
  h={'id':'machine-h','spec':{'host':'h','owner':'team','capabilities':['machine'],'resources':{'kind':'machine','metrics_stream':{'enabled':True},'templates':[{'name':'base'}]}}}
  result=reconcile([h],{'machine-h':{'checked':100,'expires':300,'entries':[{'id':'base','state':'stopped','type':'tart'}]}},now=110)
  self.assertEqual(result['findings'],[])
 def test_malformed_binding_does_not_count_as_verified_placement(self):
  r={'id':'web','spec':{'host':'h','owner':'team','resources':{'runtime_binding':'oops'}}}
  self.assertEqual(reconcile([r],{},now=100)['findings'][0]['code'],'deployment-unmapped')
