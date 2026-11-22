"""Check a candidate Proxmox CUDA LXC recipe without altering its worker."""

import argparse
import json
import time
from pathlib import Path

from checkpoints import Rejected
from pve_dynamic import DynamicScope


DEVICES = {'/dev/nvidia0', '/dev/nvidiactl', '/dev/nvidia-uvm', '/dev/nvidia-uvm-tools'}


def verify(recipe, config, observation, *, now=None):
    now = time.time() if now is None else now
    checked = observation.get('observed_at')
    if type(checked) not in (int, float) or not now - 45 <= checked <= now + 5:
        raise Rejected('cuda_observation_stale', 'Check live GPU usage and reservations before admission.')
    if (config.get('template') != 1 or config.get('unprivileged') != 1
            or config.get('arch') != 'amd64' or config.get('ostype') != 'debian'
            or DynamicScope.config_digest(config) != recipe['template_config_sha256']
            or config.get('rootfs', '').split(',')[0] != recipe['volume']):
        raise Rejected('cuda_template_changed', 'The enrolled unprivileged Debian CUDA template changed.')

    mappings = {value.split(',', 1)[0].removeprefix('path=')
                for key, value in config.items() if key.startswith('dev') and key[3:].isdigit()}
    if mappings != DEVICES or any(key.startswith(('lxc.', 'mp')) for key in config):
        raise Rejected('cuda_device_scope', 'The template has unexpected device mappings or host mounts.')
    gpu = observation['gpu']
    if (gpu['uuid'] != recipe['gpu_uuid'] or gpu['driver'] != recipe['driver']
            or type(gpu['memory_mib']) is not int or gpu['memory_mib'] < recipe['minimum_vram_mib']):
        raise Rejected('cuda_worker_changed', 'The observed GPU or driver differs from the qualified recipe.')
    if observation['compute_processes'] or observation['active_reservations']:
        raise Rejected('cuda_busy', 'The GPU has an active compute process or another reservation.')
    return {'gpu': 'cuda', 'access_mode': 'exclusive-lease-shared-host-driver',
            'vram_mib': gpu['memory_mib'], 'driver': gpu['driver'], 'observed_at': checked}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path, help='Pinned recipe and freshly collected provider observations')
    data = json.loads(parser.parse_args().input.read_text())
    try:
        result = verify(data['recipe'], data['template'], data['observation'])
    except Rejected as error:
        print(json.dumps({'code': error.code, 'reason': str(error)}))
        return 2
    print(json.dumps(result))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
