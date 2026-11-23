"""Provider-only adapter for the native controller. It never opens the lease database."""
from contextlib import redirect_stdout
import copy
import json
import re
import signal
import sys
import urllib.error
import uuid
from pathlib import Path

from capabilities import match
from checkpoints import Rejected
from pilot import MIB, SLOT_ID, Pilot, Slot, public_key

OPERATIONS = {
    'controller-generation', 'slots', 'enrollment-check', 'capabilities', 'prepare',
    'task-history', 'dispatch', 'task-state', 'clone', 'describe-clone',
    'guest-observation', 'install-key', 'shape', 'verify-shape',
    'verify-after-cleanup', 'export-source', 'reservation-capacity',
}
MUTATIONS = {
    'starting': 'start', 'stopping': 'shutdown', 'export-stopping': 'shutdown',
    'resetting': 'reset', 'deleting': 'delete', 'resizing': 'resize',
}
LEASE_ID = re.compile('[0-9a-f]{32}')


class Adapter:
    def __init__(self, config):
        self.config = config

    def slots(self):
        extra = self.config.get('additional_slots', {})
        if (not isinstance(extra, dict) or len(extra) > 7 or 'primary' in extra
                or not all(SLOT_ID.fullmatch(slot_id) for slot_id in extra)):
            raise ValueError('Invalid enrolled slot configuration')
        return {'primary': self.config, **extra}

    @staticmethod
    def bind(slot, lease, pins):
        if not slot.deletes_on_release:
            return slot.provider
        if not isinstance(pins, dict):
            raise Rejected('dynamic_outcome_unknown',
                           'The clone has not established a verifiable identity.')
        if slot.apple:
            return slot.apple.bind(lease, pins)
        provider = copy.copy(slot.provider)
        provider.name = slot.dynamic.name(lease)
        provider.uuid, provider.volume = pins['uuid'], pins['volume']
        return provider

    def invoke(self, request):
        action = request.get('action')
        if action not in OPERATIONS:
            raise Rejected('provider_operation_unknown', 'Unknown provider operation.', 400)
        definitions = self.slots()
        if action == 'controller-generation':
            slot = Slot.configure(definitions['primary'])
            vmid = int(self.config['controller_guest_id'])
            response = slot.provider.call(f'/nodes/{slot.provider.node}/qemu/{vmid}/config',
                                          token=slot.capacity.token)
            return str(uuid.UUID(response['vmgenid']))
        if action == 'slots':
            slots = {key: Slot.configure(values) for key, values in definitions.items()}
            if len({slot.provider.node for slot in slots.values()}) != len(slots):
                raise ValueError('Configure one slot per independent provider')
            return {key: {
                'identity': slot.identity(), 'recipe': slot.recipe, 'user': slot.user,
                'host_key': slot.host_key, 'node': slot.provider.node, 'guest_id': slot.provider.guest_id,
                'kind': 'tart' if slot.apple else 'dynamic' if slot.dynamic else 'fixed',
            } for key, slot in slots.items()}
        requested_slot = request.get('slot_id')
        if requested_slot not in definitions:
            raise Rejected('slot_unavailable', 'This lease has no configured slot binding.')
        slot = Slot.configure(definitions[requested_slot])
        if action == 'enrollment-check':
            Pilot.verify_idle_slot(slot)
            return True
        if action == 'capabilities':
            if slot.apple:
                observed = slot.apple.check(allow_running_slot=True)
            else:
                if slot.dynamic:
                    slot.dynamic.template_config()
                else:
                    slot.provider.verified_config()
                observed = slot.capacity.read_observation(allow_running_slot=True)
            return {**slot.platform, 'gpu': 'none', **observed['slot_resources']}
        if action in ('prepare', 'reservation-capacity'):
            reserved = tuple(request['reservations'])
            if len(reserved) != 2 or any(type(v) is not int or v < 0 for v in reserved):
                raise ValueError('Invalid reservation totals')
            if action == 'reservation-capacity':
                active = request.get('active') is True
                slot.capacity.check(reserved, require_slot=active, allow_running_slot=active)
                return True
            requirements = request['requirements']
            if slot.dynamic and not slot.dynamic.pool:
                raise Rejected('dynamic_scope_unverified', 'The slot needs its dedicated allocation pool.')
            shape = slot.dynamic.shape(requirements) if slot.dynamic else None
            capacity = slot.capacity.check(reserved, shape=shape) if shape else slot.capacity.check(reserved)
            match(requirements, capacity['slot_resources'], self.config.get('dependency_bindings', {}),
                  platform=slot.platform)
            if slot.deletes_on_release:
                Pilot.verify_idle_slot(slot)
            if slot.apple:
                phase, generation, prior = 'cloning', None, slot.apple.tasks('clone')
            elif slot.dynamic:
                phase, generation, prior = 'cloning', None, slot.dynamic.recent_clone_tasks()
            else:
                if slot.provider.status() != 'stopped':
                    raise Rejected('slot_unavailable', 'The slot is running outside the controller.')
                slot.provider.verify_clean(request.get('clean_rollback') or slot.provider.initial_rollback)
                phase, generation, prior = 'starting', slot.provider.generation(), slot.provider.recent_tasks('starting')
            return {'phase': phase, 'generation': generation, 'prior_tasks': sorted(prior),
                    'memory': capacity['memory_after_start'] - reserved[0] * MIB,
                    'storage': capacity['storage_after_writes'] - reserved[1] * MIB}

        lease = request.get('lease')
        if not isinstance(lease, str) or not LEASE_ID.fullmatch(lease):
            raise Rejected('unknown_lease', 'No lease has this identifier.', 404)
        phase = request.get('phase')
        scope = slot.apple or slot.dynamic
        pins = request.get('pins')
        if action == 'clone':
            if not scope:
                raise Rejected('slot_unavailable', 'This slot is not a clone provider.')
            Pilot.verify_idle_slot(slot)
            task = scope.clone(lease)
            valid = task == 'clone:' + lease if slot.apple else scope.matches_clone(task)
            if not valid:
                raise Rejected('operation_outcome_unknown', 'Clone task cannot be confirmed. The slot remains held.')
            return task
        if action == 'task-history' and phase == 'cloning':
            return sorted(slot.apple.tasks('clone', lease) if slot.apple else slot.dynamic.recent_clone_tasks())
        if action == 'task-state' and phase == 'cloning':
            task = request['task']
            valid = task == 'clone:' + lease if slot.apple else slot.dynamic.matches_clone(task)
            if not valid:
                raise Rejected('operation_outcome_unknown', 'The recorded clone task cannot be attributed.')
            return slot.provider.task(task)
        if action == 'describe-clone':
            return scope.describe(lease)
        if action == 'verify-after-cleanup' and phase == 'deleting':
            if slot.apple:
                slot.apple.verify_idle()
            else:
                slot.dynamic.verify_absent()
            return True

        provider = self.bind(slot, lease, pins)
        if action == 'task-history':
            return sorted(provider.recent_tasks(phase))
        if action == 'task-state':
            task = request['task']
            if not provider.matches_task(task, phase):
                raise Rejected('operation_outcome_unknown', 'The recorded provider task cannot be attributed.')
            return provider.task(task)
        if action == 'verify-after-cleanup' and phase == 'resetting':
            if provider.status() != 'stopped':
                raise Rejected('unexpected_guest_state', 'The reset slot is not stopped.')
            provider.verify_clean(request['task'])
            if provider.generation() == request.get('generation'):
                raise Rejected('slot_reconciliation_required', 'Rollback has not advanced the slot generation.')
            return True
        if provider.generation() != request.get('generation'):
            raise Rejected('slot_reconciliation_required',
                           'Provider generation differs from the recorded lease. Hold for reconciliation.')
        if action == 'dispatch':
            if phase not in MUTATIONS:
                raise Rejected('provider_operation_unknown', 'Unknown provider mutation.', 400)
            if phase == 'deleting':
                if not scope or scope.describe(lease) != pins:
                    raise Rejected('dynamic_identity_changed', 'Guest or disk identity changed before deletion.')
                task = provider.delete(discard=request.get('discard') is True) if slot.apple else provider.delete()
            elif phase == 'resizing':
                task = slot.dynamic.resize(lease, pins, slot.dynamic.shape(request['requirements']))
            else:
                task = getattr(provider, MUTATIONS[phase])()
            if not provider.matches_task(task, phase):
                raise Rejected('operation_outcome_unknown',
                               'The provider returned no matching task identifier. The slot remains held.')
            return task
        if action == 'guest-observation':
            power = provider.status()
            address = None
            if request.get('include_address', True) and power == 'running':
                try:
                    address = provider.ip()
                except urllib.error.HTTPError as error:
                    if error.code != 500:
                        raise
            return {'status': power, 'address': address, 'name': provider.name}
        if action == 'install-key':
            host_key = provider.ready_host_key() if slot.deletes_on_release else slot.host_key
            if not host_key:
                return None
            provider.install_guest_key(public_key(request['ssh_key']), lease)
            return host_key
        if action in ('shape', 'verify-shape'):
            shape = slot.dynamic.shape(request['requirements'])
            if action == 'verify-shape':
                slot.dynamic.config_matches(lease, pins, shape, check_disk=True)
                return True
            return {'current_disk_mib': slot.dynamic.set_shape(lease, pins, shape),
                    'disk_mib': shape['disk_mib']}
        if action == 'export-source':
            if provider.status() != 'stopped':
                raise Rejected('export_source_changed', 'The guest is not stopped. Preserve its disk for review.')
            tasks = json.loads(provider.slot_start_tasks())
            if 'start_tasks' in request and request['start_tasks'] != tasks:
                raise Rejected('export_source_changed', 'The guest start history changed. Preserve its disk for review.')
            source = {'start_tasks': tasks}
            if slot.dynamic:
                size = slot.dynamic.shape(request['requirements'])['disk_mib']
                source['dynamic_source'] = [pins[k] for k in ('uuid', 'generation', 'volume')] + [str(size)]
            elif slot.apple:
                source['workspace_source'] = request['generation']
            return source
        raise Rejected('provider_operation_unknown', 'Unknown provider operation.', 400)


def main():
    try:
        signal.alarm(50)
        if len(sys.argv) != 2:
            raise ValueError('Expected the configured allocator settings path')
        config = json.loads(Path(sys.argv[1]).read_text())
        raw = sys.stdin.buffer.read(128 * 1024 + 1)
        if len(raw) > 128 * 1024:
            raise ValueError('Provider request was too large')
        request = json.loads(raw)
        with redirect_stdout(sys.stderr):
            result = Adapter(config).invoke(request)
        response = {'result': result}
    except Rejected as error:
        response = {'error': {'code': error.code, 'reason': error.reason, 'status': error.status}}
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        print(f'Provider adapter failed: {type(error).__name__}', file=sys.stderr)
        response = {'error': {'code': 'provider_unavailable',
                              'reason': 'Check the recorded lease before retrying.', 'status': 502}}
    finally:
        signal.alarm(0)
    print(json.dumps(response))


if __name__ == '__main__':
    main()
