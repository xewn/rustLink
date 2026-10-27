"""Compare original PLC libraries against Rust on the same actual simulated sockets."""
import asyncio
import json
import logging
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import types
import httpx
import websockets

root=Path(__file__).resolve().parents[1]
source=Path(sys.argv[1]) if len(sys.argv)>1 else Path('D:/PLCLink/FactoryLink')
sys.dont_write_bytecode=True;sys.path.insert(0,str(source/'backend'))
stub=types.ModuleType('logger');stub.gateway_logger=types.SimpleNamespace(get_logger=lambda *a:logging.getLogger('oracle'));stub.logger=logging.getLogger('oracle');sys.modules['logger']=stub
from collector.modbus import ModbusCollector
from collector.s7 import S7Collector
from collector.mitsubishi import MitsubishiCollector
from snap7 import Client

async def main():
    processes=[]
    try:
        suffix='.exe' if os.name=='nt' else ''
        simulator=subprocess.Popen([str(root/f'target/release/examples/simulator{suffix}')],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0));processes.append(simulator)
        async with httpx.AsyncClient(timeout=5) as client:
            for _ in range(100):
                try:await client.post('http://127.0.0.1:18080/v1/chat/completions',json={});break
                except httpx.HTTPError:await asyncio.sleep(.05)
            assert simulator.poll() is None,'Simulator could not bind its local ports'
            config=json.loads((root/'examples/config.simulated.json').read_text(encoding='utf-8'))
            originals={}
            for d in config['devices']:
                cls={'modbus_tcp':ModbusCollector,'s7':S7Collector,'mitsubishi':MitsubishiCollector}[d['protocol']]
                collector=cls(d,{'max_retries':1})
                if d['protocol']=='s7':
                    # Original wrapper ignores configurable port; only redirect its client to the test port.
                    collector._client=Client().connect('127.0.0.1',0,1,tcp_port=1102)
                else:assert collector._do_connect()
                collector._connected=True
                data=collector.read();data.pop('timestamp');data['gateway']=config['gateway_name'];originals[d['id']]=data
                assert all(v['quality']=='good' for v in data['values'].values()),data
                collector.disconnect()
            with tempfile.TemporaryDirectory(prefix='rustlink-system-') as temp:
                (Path(temp)/'config.json').write_text(json.dumps(config,ensure_ascii=False),encoding='utf-8')
                with socket.socket() as socket_:socket_.bind(('127.0.0.1',0));port=socket_.getsockname()[1]
                gateway=subprocess.Popen([str(root/f'target/release/rustlink{suffix}'),'--headless','--port',str(port),'--strict-port','--data-dir',temp],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0));processes.append(gateway)
                base=f'http://127.0.0.1:{port}'
                try:
                    for _ in range(100):
                        try:await client.get(base+'/api/health');break
                        except httpx.HTTPError:await asyncio.sleep(.05)
                    async with websockets.connect(f'ws://127.0.0.1:{port}/ws/data') as ws:
                        await client.post(base+'/api/collect/start');observed={}
                        async with asyncio.timeout(10):
                            async for raw in ws:
                                v=json.loads(raw)
                                if v.get('type')=='data' and all(p['quality']=='good' for p in v['values'].values()):
                                    v.pop('type');v.pop('timestamp');assert v==originals[v['device_id']],(v,originals[v['device_id']]);observed[v['device_id']]=v
                                    if len(observed)==3:break
                    for d in config['devices']:
                        tested=(await client.post(base+'/api/devices/test',json={'device':d})).json();assert tested['success'];assert tested['message']==f"连接成功，耗时 {tested['elapsed_ms']}ms"
                    ai=(await client.post(base+'/api/ai/parse',json={'input':'模拟PLC'})).json();assert ai['success'] and ai['data']['points'][0]['rate']==1.0
                    stream=(await client.post(base+'/api/ai/parse-stream',json={'input':'模拟PLC'})).text;assert '"type":"done"' in stream and '"type":"chunk"' in stream
                    status=(await client.get(base+'/api/collect/status')).json();assert status['data']['mqtt_connected']
                    artifacts=root/'artifacts';artifacts.mkdir(exist_ok=True);(artifacts/'differential.json').write_text(json.dumps({'baseline':str(source),'matched_devices':observed,'mqtt_connected':True,'ai_json_and_sse':True},ensure_ascii=False,indent=2),encoding='utf-8')
                    print('PASS: original pymodbus, python-snap7, pymcprotocol and Rust produce identical PLC payloads; MQTT, AI JSON/SSE and connection-test messages verified')
                finally:gateway.terminate();gateway.wait(timeout=10)
    finally:
        for process in reversed(processes):
            if process.poll() is None:process.terminate();process.wait(timeout=10)
asyncio.run(main())
