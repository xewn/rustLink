"""Capture real FactoryLink behavior without accessing PLCs or modifying its tree."""
import argparse
import hashlib
import importlib.util
import json
import logging
from pathlib import Path
import random
import struct
import sys
import types

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, default=Path('D:/PLCLink/FactoryLink'))
parser.add_argument('--output', type=Path, default=Path('tests/fixtures/oracle.json'))
args = parser.parse_args()
backend = args.source / 'backend'
sys.dont_write_bytecode = True
sys.path.insert(0, str(backend))
logger = types.ModuleType('logger')
logger.gateway_logger = types.SimpleNamespace(get_logger=lambda *a: logging.getLogger('oracle'))
logger.logger = logging.getLogger('oracle')
sys.modules['logger'] = logger
for name in ['pymodbus', 'pymodbus.client', 'pymodbus.exceptions']:
    sys.modules[name] = types.ModuleType(name)
sys.modules['pymodbus.client'].ModbusTcpClient = object
sys.modules['pymodbus.client'].ModbusSerialClient = object
sys.modules['pymodbus.exceptions'].ConnectionException = Exception

from collector.modbus import ModbusCollector
from collector.s7 import S7Collector
from collector.mitsubishi import MitsubishiCollector
from schemas import DeviceConfig, PointConfig, GatewayConfig, MqttConfig, AIConfig

modbus = ModbusCollector({}, {})
s7 = S7Collector({}, {})
mc = MitsubishiCollector({}, {})
random.seed(20261027)
out = {'source': str(args.source), 'sha256': {}, 'modbus_addresses': [],
       's7_addresses': [], 'decode': [], 'bits': [], 'mc': [], 'scaling': [], 'models': []}
for f in sorted(backend.rglob('*.py')):
    out['sha256'][f.relative_to(backend).as_posix()] = hashlib.sha256(f.read_bytes()).hexdigest()
for address in ['0','1','00001','9999','10000','10001','19999','20000','30000','30001','39999','40000','40001','49999','50000','65535','-1',' 40001 ','abc','']:
    try: result = list(modbus._parse_address(address))
    except Exception: result = None
    out['modbus_addresses'].append({'input': address, 'expected': result})
for address in ['DB1.DBD0','db2.dbw20','DB1.DBX3.7','DB3.DBB1','DB0.DBX0','DB1.DBD0.2','M0.0','i10.1','Q2','E3.7','a4.2','M0.8','DB1.DBX0.99',' DB1.DBD0','DB1.DBD-1','MW0','V1','']:
    try: result = list(s7._parse_address(address))
    except Exception: result = None
    out['s7_addresses'].append({'input': address, 'expected': result})
types_ = ['bool','int16','uint16','int32','uint32','float','double','unknown']
for dtype in types_:
    samples = [[0,0,0,0], [65535]*4, [32768,0,0,0], [16844,0,0,0]]
    samples += [[random.randrange(65536) for _ in range(4)] for _ in range(48)]
    samples += [[],[1],[1,2],[1,2,3]]
    for registers in samples:
        for protocol in ['modbus','s7']:
            raw = b''.join(struct.pack('>H', x) for x in registers)
            try:
                value = modbus._decode_registers(registers,dtype) if protocol == 'modbus' else s7._decode_data(raw,dtype)
                if isinstance(value,float) and not (-float('inf') < value < float('inf')): continue
            except Exception: value = None
            out['decode'].append({'protocol':protocol,'type':dtype,'bytes':list(raw),'expected':value})
for dtype in types_:
    for bits in [[],[False],[True],[True,False,True],[True]*16,[False]*15+[True],[True]*20]:
        out['bits'].append({'type':dtype,'bits':bits,'expected':modbus._decode_bits(bits,dtype)})
class FakeMC:
    def __init__(self, words): self.words = words
    def batchread_wordunits(self,*args): return self.words
    def batchread_bitunits(self,*args): return self.words
for dtype in types_:
    for words in [[0]*4,[1,2,3,4],[-1]*4,[-32768,0,0,0],[0,16844,0,0],[]]:
        mc._client = FakeMC(words)
        point = {'name':'测试','address':'D0','type':dtype,'rate':1.5,'offset':-2,'unit':'℃'}
        value = mc._do_read(point)
        if value and isinstance(value['value'],float) and not (-float('inf') < value['value'] < float('inf')): continue
        out['mc'].append({'type':dtype,'words':words,'rate':1.5,'offset':-2,'expected':value['value'] if value else None})
for dtype, raw in [('bool',True),('bool',False),('int16',-42),('float',25.5)]:
    for rate, offset in [(1.,0.),(.1,-3.),(-2.,10.),(0.,9.)]:
        modbus._read_register = lambda *a, v=raw: v
        result=modbus._do_read({'name':'p','address':'40001','type':dtype,'rate':rate,'offset':offset})
        out['scaling'].append({'raw':raw,'rate':rate,'offset':offset,'expected':result['value']})
for cls, data in [(PointConfig,{'name':'温度','address':'40001'}),(DeviceConfig,{'id':'d','name':'设备','protocol':'modbus_tcp'}),(DeviceConfig,{'id':'s','name':'串口','protocol':'modbus_rtu','port':'COM4','parity':None}),(GatewayConfig,{}),(MqttConfig,{}),(AIConfig,{})]:
    out['models'].append({'model':cls.__name__,'input':data,'expected':cls(**data).model_dump()})
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(out,ensure_ascii=False,indent=2,allow_nan=False)+'\n',encoding='utf-8')
print({k:len(v) for k,v in out.items() if isinstance(v,list)})
