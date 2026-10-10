"""Separate catalog entities and their observations; never infer health from capacity."""
from dashboard import public_record
from access import public_access

def entity_type(record):
    spec=record['spec'];kind=spec.get('resources',{}).get('kind')
    if kind=='machine' or 'machine' in spec.get('capabilities',[]):return 'machine'
    if kind=='historical-evidence' or 'inventory-history' in spec.get('capabilities',[]):return 'archive'
    return 'service'

def inventory(records,metrics):
    machines=[];services=[]
    names={r['spec']['host']:r['id'] for r in records if entity_type(r)=='machine'}
    for r in records:
        kind=entity_type(r)
        if kind=='archive':continue
        item=public_record(r)
        if kind=='machine':
            res=r['spec'].get('resources',{});hw=res.get('hardware',{})
            item.update(access=public_access(r),console_login=res.get('console_login'),kind='machine',model=res.get('model'),cpu=hw.get('cpu'),cores=hw.get('cores'),ram_mib=hw.get('memory_mib'),os=res.get('os',{}).get('name'),metrics=metrics.get(r['id'],{'status':'unconfigured'}))
            machines.append(item)
        else:
            item.update(kind='service',machine_id=names.get(item['host']),deployment=r['spec'].get('deployment',''))
            services.append(item)
    return {'machines':machines,'services':services}
