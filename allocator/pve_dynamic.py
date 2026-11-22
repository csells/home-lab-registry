"""Proxmox clone lifecycle within one enrolled staging guest ID and storage scope."""
import hashlib
import json
import re
import subprocess
import urllib.parse
import uuid

from checkpoints import Rejected

IDENTIFIER = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,63}')
LEASE_ID = re.compile(r'[0-9a-f]{32}')


def uuid_field(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f-]{36}', value):
        raise ValueError('Expected a canonical provider UUID')
    parsed = uuid.UUID(value)
    if parsed.int == 0 or str(parsed) != value:
        raise ValueError('Expected a canonical provider UUID')
    return value


class DynamicScope:
    def __init__(self, connection, settings, storage):
        self.api = connection
        self.node, self.guest_id = connection.node, connection.guest_id
        self.storage = storage
        self.pool = settings.get('pool')
        self.inventory_command = settings.get('inventory_command')
        if (self.pool is not None and not IDENTIFIER.fullmatch(self.pool)
                or self.inventory_command is not None and (not isinstance(self.inventory_command, list)
                    or not self.inventory_command or not all(isinstance(v, str) and v for v in self.inventory_command))):
            raise ValueError('Use the exact private provider scope and inventory command')
        self.template = str(int(settings['template_id']))
        self.template_name = settings['template_name']
        self.template_uuid = uuid_field(settings['template_uuid'])
        self.template_volume = settings['template_volume']
        self.bounds = settings['maximum']
        self.default = settings['default']
        self.qualification = settings.get('qualification')
        keys = {'vcpus', 'memory_mib', 'disk_mib'}
        if (not IDENTIFIER.fullmatch(storage) or not IDENTIFIER.fullmatch(self.template_name)
                or self.template == self.guest_id
                or not self.template_volume.startswith(storage + ':')
                or self.bounds.keys() != keys or self.default.keys() != keys
                or not all(type(self.default[k]) is int and type(self.bounds[k]) is int
                           and 0 < self.default[k] <= self.bounds[k] for k in keys)
                or self.default['disk_mib'] % 1024 or self.bounds['disk_mib'] % 1024):
            raise ValueError('Pin a distinct template, storage and bounded CPU, memory and disk recipe')
        self.boot_volume = re.compile(
            re.escape(storage) + r':' + re.escape(self.guest_id) + r'/vm-' +
            re.escape(self.guest_id) + r'-disk-[0-9]+\.raw')
        if self.qualification is not None:
            self.validate_qualification(self.qualification)

    @staticmethod
    def validate_qualification(qualification):
        if (not isinstance(qualification, dict)
                or set(qualification) != {'image_sha256', 'bootstrap_revision', 'template_config_sha256'}
                or any(not isinstance(qualification[key], str)
                       or not re.fullmatch(r'[a-f0-9]{64}', qualification[key])
                       for key in ('image_sha256', 'template_config_sha256'))
                or not isinstance(qualification['bootstrap_revision'], str)
                or not IDENTIFIER.fullmatch(qualification['bootstrap_revision'])):
            raise ValueError('Qualify the recipe with image and template digests and a bootstrap revision')

    @staticmethod
    def config_digest(config):
        data = {key: value for key, value in config.items() if key not in ('digest', 'lock')}
        return hashlib.sha256(json.dumps(data, sort_keys=True, separators=(',', ':')).encode()).hexdigest()

    @staticmethod
    def network(config):
        if ({key for key in config if re.fullmatch(r'net[0-9]+', key)} != {'net0'}
                or {key for key in config if re.fullmatch(r'ipconfig[0-9]+', key)} != {'ipconfig0'}
                or config.get('ipconfig0') != 'ip=dhcp'):
            raise Rejected('recipe_network_unverified', 'The recipe needs a single DHCP interface.')
        interface = config['net0'].split(',')
        if not re.fullmatch(r'virtio=(?:[a-fA-F0-9]{2}:){5}[a-fA-F0-9]{2}', interface[0]):
            raise Rejected('recipe_network_unverified', 'The recipe needs a pinned virtio network configuration.')
        options = sorted(interface[1:])
        if (sum(field.startswith('bridge=') for field in options) != 1
                or any(field == 'link_down=1' for field in options)):
            raise Rejected('recipe_network_unverified', 'The recipe requires an active enrolled bridge.')
        return options

    def preflight_storage(self, storage_status):
        if self.qualification is None:
            return
        if (storage_status.get('type') != 'dir'
                or 'images' not in storage_status.get('content', '').split(',')):
            raise Rejected('recipe_storage_unverified',
                           'The qualified raw full-clone recipe requires directory storage with VM images enabled.')

    def identity(self):
        identity = [self.api.base, self.node, self.guest_id, self.api.identity, self.storage,
                self.template, self.template_name, self.template_uuid, self.template_volume, self.default, self.bounds]
        if self.pool:
            identity.append(self.pool)
        if self.qualification is not None:
            identity.append(self.qualification)
        return identity

    def template_config(self):
        config = self.api.call(f'/nodes/{self.node}/qemu/{self.template}/config')
        if (config.get('name') != self.template_name or config.get('template') != 1
                or config.get('ipconfig0') != 'ip=dhcp' or config.get('lock')
                or 'uuid=' + self.template_uuid not in config.get('smbios1', '').split(',')
                or config.get('scsi0', '').split(',')[0] != self.template_volume
                or 'size=' + str(self.default['disk_mib'] // 1024) + 'G' not in config.get('scsi0', '').split(',')):
            raise Rejected('template_identity_changed', 'The enrolled clean DHCP template could not be verified.')
        if self.qualification is not None:
            if self.config_digest(config) != self.qualification['template_config_sha256']:
                raise Rejected('template_identity_changed', 'The versioned template configuration changed.')
            self.network(config)
        return config

    def storage_files(self):
        if not self.inventory_command:
            raise Rejected('dynamic_storage_unverified', 'Use the enrolled physical storage inventory command.')
        try:
            result = subprocess.check_output(self.inventory_command, stderr=subprocess.DEVNULL, timeout=8)
            files = json.loads(result)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            raise Rejected('dynamic_storage_unverified', 'Owned physical volume inventory could not be verified.') from error
        if (not isinstance(files, list) or len(files) > 8
                or any(not isinstance(item, dict) or set(item) != {'volid'}
                       or not isinstance(item['volid'], str) for item in files)):
            raise Rejected('dynamic_storage_unverified', 'The owned physical volume inventory was invalid.')
        return files

    def verify_absent(self):
        inventory = self.api.call('/cluster/resources?type=vm', token=self.api.audit_token)
        if any(str(row['vmid']) == self.guest_id for row in inventory) or self.storage_files():
            raise Rejected('dynamic_scope_occupied', 'The enrolled guest ID or its scoped storage is occupied.')
        if self.pool:
            pool = self.api.call('/pools/' + urllib.parse.quote(self.pool, safe=''))
            if pool.get('members') != []:
                raise Rejected('dynamic_scope_occupied', 'The allocation pool has unexpected members.')

    def shape(self, request):
        shape = {k: max(value, request.get(k, value)) for k, value in self.default.items()}
        shape['disk_mib'] = ((shape['disk_mib'] + 1023) // 1024) * 1024
        return shape

    @staticmethod
    def disk_mib(config):
        sizes = [field.removeprefix('size=') for field in config['scsi0'].split(',') if field.startswith('size=')]
        if len(sizes) != 1 or not re.fullmatch(r'[1-9][0-9]*G', sizes[0]):
            raise Rejected('dynamic_shape_unverified', 'The provider boot volume size was not verified.')
        return int(sizes[0][:-1]) * 1024

    def name(self, lease):
        if not isinstance(lease, str) or not LEASE_ID.fullmatch(lease):
            raise ValueError('Invalid dynamic guest lease identity')
        return 'lease-' + lease

    def clone(self, lease):
        if not self.pool:
            raise Rejected('dynamic_scope_unverified', 'Enroll a dedicated pool before allocating new clones.')
        self.template_config()
        self.verify_absent()
        return self.api.call(f'/nodes/{self.node}/qemu/{self.template}/clone',
                             {'newid': self.guest_id, 'name': self.name(lease), 'full': 1,
                              'format': 'raw', 'storage': self.storage, 'pool': self.pool})

    def matches_clone(self, upid):
        parts = upid.split(':') if isinstance(upid, str) else []
        return (len(parts) == 9 and parts[0] == 'UPID' and parts[1] == self.node
                and parts[5:8] == ['qmclone', self.template, self.api.identity])

    def recent_clone_tasks(self):
        query = urllib.parse.urlencode({'source': 'all', 'vmid': self.template,
                                        'typefilter': 'qmclone', 'limit': 50})
        tasks = self.api.call(f'/nodes/{self.node}/tasks?{query}')
        return {row['upid'] for row in tasks if self.matches_clone(row.get('upid'))}

    def assert_disks(self, config):
        # A task must never acquire provider storage permissions. Reject unexpected
        # disks even if someone edited the guest outside the allocator.
        disks = []
        cloud_init = None
        for key, value in config.items():
            if not re.fullmatch(r'(?:scsi|sata|ide|virtio|unused|efidisk|tpmstate)[0-9]+', key):
                continue
            volume = value.split(',')[0]
            if volume == 'none':
                continue
            if key == 'ide2' and volume == f'{self.storage}:{self.guest_id}/vm-{self.guest_id}-cloudinit.raw':
                cloud_init = volume
                continue
            if not self.boot_volume.fullmatch(volume):
                raise Rejected('unowned_volume', 'The guest contains a volume outside its enrolled staging scope.')
            disks.append((key, volume))
        if len(disks) != 1 or disks[0][0] != 'scsi0':
            raise Rejected('unowned_volume', 'The dynamic recipe requires one enrolled boot volume.')
        volume = disks[0][1]
        files = self.storage_files()
        owned = {entry['volid'] for entry in files}
        if cloud_init:
            owned.discard(cloud_init)
        if owned != {volume}:
            raise Rejected('unowned_volume', 'Scoped storage does not match the guest’s attached boot volume.')
        return volume

    def describe(self, lease):
        if self.pool:
            membership = self.api.call('/pools/' + urllib.parse.quote(self.pool, safe='')).get('members', [])
            if (len(membership) != 1 or membership[0].get('type') != 'qemu'
                    or str(membership[0].get('vmid')) != self.guest_id
                    or membership[0].get('node') != self.node):
                raise Rejected('dynamic_identity_changed', 'The allocation pool does not contain exactly the owned clone.')
        config = self.api.call(self.api.guest + '/config')
        if (config.get('name') != self.name(lease) or config.get('template') or config.get('lock')
                or config.get('ipconfig0') != 'ip=dhcp'):
            raise Rejected('dynamic_identity_changed', 'The allocated clone failed exact identity or DHCP verification.')
        if self.qualification is not None and self.network(config) != self.network(self.template_config()):
            raise Rejected('recipe_network_unverified', 'The clone network differs from the enrolled template.')
        hardware = [item.removeprefix('uuid=') for item in config['smbios1'].split(',') if item.startswith('uuid=')]
        if len(hardware) != 1 or uuid_field(hardware[0]) == self.template_uuid:
            raise Rejected('dynamic_identity_changed', 'The clone lacks an independent hardware identity.')
        volume = self.assert_disks(config)
        return {'uuid': hardware[0], 'volume': volume, 'generation': uuid_field(config.get('vmgenid'))}

    def config_matches(self, lease, pins, shape, *, check_disk):
        current = self.api.call(self.api.guest + '/config')
        if (self.describe(lease) != pins or int(current['memory']) != shape['memory_mib']
                or int(current['cores']) != shape['vcpus'] or current.get('lock')
                or check_disk and self.disk_mib(current) != shape['disk_mib']):
            raise Rejected('dynamic_shape_unverified', 'The provider did not confirm the requested clone resources.')
        return self.disk_mib(current)

    def set_shape(self, lease, pins, shape):
        if self.describe(lease) != pins:
            raise Rejected('dynamic_identity_changed', 'The clone identity changed before sizing.')
        self.api.call(self.api.guest + '/config',
                      {'memory': shape['memory_mib'], 'cores': shape['vcpus'], 'sockets': 1,
                       'balloon': 0}, method='PUT')
        return self.config_matches(lease, pins, shape, check_disk=False)

    def resize(self, lease, pins, shape):
        old = self.config_matches(lease, pins, shape, check_disk=False)
        if old >= shape['disk_mib']:
            raise Rejected('dynamic_shape_unverified', 'Unexpected disk size before the enrolled growth operation.')
        return self.api.call(self.api.guest + '/resize',
                             {'disk': 'scsi0', 'size': str(shape['disk_mib'] // 1024) + 'G'}, method='PUT')
