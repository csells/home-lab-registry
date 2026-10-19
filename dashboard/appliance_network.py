"""Read-only appliance settings adapters. Auth material never enters result rows."""
import http.client,hashlib,hmac,http.cookiejar,io,ipaddress,json,logging,pathlib,re,ssl,urllib.request,urllib.parse,uuid,zipfile
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

class PinnedHTTPSConnection(http.client.HTTPSConnection):
    """Authenticate an appliance's exact provisioned certificate before any HTTP data."""
    def __init__(self,host,*,certificate,**kwargs):
        self.expected=hashlib.sha256(ssl.PEM_cert_to_DER_cert(certificate)).digest()
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname=False
        context.verify_mode=ssl.CERT_NONE  # Exact certificate pin is enforced in connect, before login.
        kwargs.pop('context',None);kwargs.pop('check_hostname',None)
        super().__init__(host,context=context,**kwargs)
    def connect(self):
        super().connect()
        actual=hashlib.sha256(self.sock.getpeercert(binary_form=True)).digest()
        if not hmac.compare_digest(actual,self.expected):
            self.close()
            raise ssl.SSLCertVerificationError('Appliance certificate changed; reverify its identity')

class PinnedHTTPSHandler(urllib.request.HTTPSHandler):
    def __init__(self,certificate):
        super().__init__();self.certificate=certificate
    def https_open(self,req):
        return self.do_open(lambda host,**kw:PinnedHTTPSConnection(host,certificate=self.certificate,**kw),req)

def glinet_rows(value):
    if not value.get('ok') or not value.get('result',{}).get('success'):raise ValueError('GLKVM rejected network read')
    c=value['result']['config']
    if not isinstance(c.get('is_dhcp'),bool):raise ValueError('Missing assignment mode')
    return [{'address':c['ip_address'],'mac':c['mac_address'].lower(),'assignment':'dhcp' if c['is_dhcp'] else 'static','source':'Authenticated GLKVM network settings'}]

def amt_rows(url,credentials):
    passwords=urllib.request.HTTPPasswordMgrWithDefaultRealm();passwords.add_password(None,url,credentials.get('username','admin'),credentials['password'])
    opener=urllib.request.build_opener(urllib.request.HTTPDigestAuthHandler(passwords))
    enumeration='http://schemas.xmlsoap.org/ws/2004/09/enumeration';resource='http://intel.com/wbem/wscim/1/amt-schema/1/AMT_EthernetPortSettings'
    def call(action,body):
        xml=f'''<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:w="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd" xmlns:e="{enumeration}"><s:Header><a:To>http://127.0.0.1:16992/wsman</a:To><a:ReplyTo><a:Address>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:Address></a:ReplyTo><a:Action s:mustUnderstand="true">{enumeration}/{action}</a:Action><w:ResourceURI s:mustUnderstand="true">{resource}</w:ResourceURI><a:MessageID>uuid:{uuid.uuid4()}</a:MessageID><w:OperationTimeout>PT60S</w:OperationTimeout></s:Header><s:Body>{body}</s:Body></s:Envelope>'''
        root=ET.fromstring(opener.open(urllib.request.Request(url,xml.encode(),{'Content-Type':'application/soap+xml;charset=UTF-8'}),timeout=20).read())
        if root.find('.//{*}Fault') is not None:raise ValueError('AMT SOAP fault')
        return root
    root=call('Enumerate','<e:Enumerate><w:OptimizeEnumeration/><w:MaxElements>20</w:MaxElements></e:Enumerate>');rows=[]
    for _ in range(5):
        for item in root.findall('.//{*}AMT_EthernetPortSettings'):
            fields={v.tag.split('}')[-1]:v.text for v in item}
            if fields.get('MACAddress')=='00-00-00-00-00-00':continue
            if not fields.get('IPAddress'):raise ValueError('Configured AMT interface has no address evidence')
            mode=fields.get('DHCPEnabled')
            if mode not in ('true','false'):raise ValueError('Unknown AMT assignment mode')
            rows.append({'address':fields['IPAddress'],'mac':fields['MACAddress'].replace('-',':').lower(),'assignment':'dhcp' if mode=='true' else 'static','source':'Authenticated AMT EthernetPortSettings'})
        context=root.find('.//{*}EnumerationContext')
        if root.find('.//{*}EndOfSequence') is not None or context is None:break
        root=call('Pull','<e:Pull><e:EnumerationContext>'+escape(context.text or '')+'</e:EnumerationContext><e:MaxElements>20</e:MaxElements></e:Pull>')
    else:raise ValueError('AMT enumeration did not finish')
    if not rows:raise ValueError('No AMT interfaces returned')
    return rows

def collect_appliance(target,private,credentials):
    try:
        url=target['url'].rstrip('/');host=urllib.parse.urlsplit(url).hostname
        if not ipaddress.ip_address(host).is_private:raise ValueError('Appliance endpoint must be private LAN')
        if target['type']=='amt':rows=amt_rows(url+'/wsman',credentials)
        else:
            handlers=[urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())]
            if target.get('ca_file'):
                handlers.append(PinnedHTTPSHandler((private/target['ca_file']).read_text()))
            opener=urllib.request.build_opener(*handlers)
            if target['type']=='glinet':
                body=urllib.parse.urlencode({'user':'admin','passwd':credentials['password']}).encode()
                opener.open(urllib.request.Request(url+'/api/auth/login',body),timeout=15).read()
                rows=glinet_rows(json.load(opener.open(url+'/api/system/get_network_config',timeout=15)))
            else:
                opener.open(urllib.request.Request(url+'/auth/login-local',json.dumps({'password':credentials['password']}).encode(),{'Content-Type':'application/json'}),timeout=15).read()
                raw=opener.open(url+'/diagnostics',timeout=30).read(20*1024*1024+1)
                if len(raw)>20*1024*1024:raise ValueError('JetKVM diagnostics exceeds bounded read')
                z=zipfile.ZipFile(io.BytesIO(raw));config=json.loads(z.read('config.json'));network=config['network_config']
                diagnostic=z.read('system-diagnostics.txt').decode(errors='replace')
                mac=target['mac'].lower()
                if mac not in diagnostic.lower():raise ValueError('JetKVM diagnostics did not establish expected interface MAC')
                mode=network.get('ipv4_mode')
                if mode not in ('dhcp','static'):raise ValueError('Unknown JetKVM assignment mode')
                rows=[{'address':host,'mac':mac,'assignment':mode,'source':'Authenticated JetKVM diagnostics and saved network configuration'}]
        warning=None
        if target.get('ca_file'):
            from cryptography import x509
            from datetime import datetime,timezone
            certificate=x509.load_pem_x509_certificate((private/target['ca_file']).read_bytes())
            if certificate.not_valid_after_utc<datetime.now(timezone.utc):warning='Appliance TLS certificate expired; identity verified by exact certificate pin. Renew its certificate.'
        for row in rows:
            row['machine']=target['id']
            if warning:row['warning']=warning
        return rows
    except Exception as exc:
        logging.exception('Appliance read failed: %s',target['id'])
        return [{'machine':target['id'],'status':'unverified','reason':'Appliance settings read failed: '+type(exc).__name__}]
