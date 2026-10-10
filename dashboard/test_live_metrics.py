import unittest,json,tempfile,pathlib
from unittest.mock import patch
from concurrent.futures import ThreadPoolExecutor
from metrics import LiveMetrics
class LiveMetricsTest(unittest.TestCase):
 def test_viewers_share_sampling_and_history(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=pathlib.Path(tmp);(p/'targets.json').write_text(json.dumps({'machine-test':{'kind':'proxmox'}}));live=LiveMetrics(p)
   with patch('metrics.time.time',return_value=100),patch('metrics.collect',return_value={'status':'available','cpu_percent':12}) as collect:
    with ThreadPoolExecutor(max_workers=4) as pool:results=list(pool.map(lambda _:live.read('machine-test'),range(4)))
    self.assertEqual(collect.call_count,1);self.assertTrue(all(len(x['history'])==1 for x in results))
   with patch('metrics.time.time',return_value=116),patch('metrics.collect',return_value={'status':'unavailable'}):
    result=live.read('machine-test');self.assertEqual(result['status'],'unavailable');self.assertEqual(len(result['history']),2)
   with self.assertRaises(KeyError):live.read('arbitrary-host')
 def test_history_is_bounded(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=pathlib.Path(tmp);(p/'targets.json').write_text('{"machine-test":{}}');live=LiveMetrics(p)
   with patch('metrics.collect',return_value={'status':'available'}):
    for n in range(130):
     with patch('metrics.time.time',return_value=n*16):result=live.read('machine-test')
   self.assertEqual(len(result['history']),120)
if __name__=='__main__':unittest.main()
