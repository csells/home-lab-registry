"""Separate catalog entities and their observations; never infer health from capacity."""
from dashboard import public_record
from access import public_access

def entity_type(record):
    spec=record['spec'];kind=spec.get('resources',{}).get('kind')
    if kind in ('machine','virtual-machine','container') or 'machine' in spec.get('capabilities',[]):return 'machine'
    if kind=='historical-evidence' or 'inventory-history' in spec.get('capabilities',[]):return 'archive'
    if kind=='backup-job':return 'backup'
    return 'service'

def inventory(records,metrics):
    machines=[];services=[];backups=[]
    names={r['spec']['host']:r['id'] for r in records if entity_type(r)=='machine' and r['spec'].get('resources',{}).get('kind') not in ('virtual-machine','container')}
    for r in records:
        kind=entity_type(r)
        if kind=='archive' or r['spec'].get('lifecycle')=='retired':continue
        item=public_record(r)
        res=r['spec'].get('resources',{})
        runtime=res.get('runtime',{})
        item['runtime']={k:runtime[k] for k in ('state','checked') if k in runtime}
        item['recorded_state']=res.get('state')
        item['deployment']=r['spec'].get('deployment','')
        if kind=='machine':
            res=r['spec'].get('resources',{});hw=res.get('hardware',{})
            item['display_name']=r['spec']['name'] if res.get('kind') in ('virtual-machine','container') else r['spec']['host']
            item.update(access=public_access(r),console_login=res.get('console_login'),kind='machine',model=res.get('model'),cpu=hw.get('cpu'),cores=hw.get('cores'),ram_mib=hw.get('memory_mib'),os=res.get('os',{}).get('name'),metrics=metrics.get(r['id'],{'status':'unconfigured'}))
            machines.append(item)
        elif kind=='backup':
            item.update(kind='backup',machine_id=res.get('machine_id') or names.get(item['host']))
            backups.append(item)
        else:
            item.update(kind='service',machine_id=res.get('machine_id') or names.get(item['host']),deployment=r['spec'].get('deployment',''))
            services.append(item)
    return {'machines':machines,'services':services,'backups':backups}
