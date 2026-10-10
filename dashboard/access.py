"""Public login metadata and private, allowlisted credential lookup."""
import json

def public_access(record):
    return record['spec'].get('resources',{}).get('management_access',[])

def credential(private,machine,login):
    # Only explicitly provisioned mappings; never interpret registry paths or client paths.
    data=json.loads((private/'machine-passwords.json').read_text())
    value=data.get(machine,{}).get(login)
    return value if isinstance(value,str) and value else None
