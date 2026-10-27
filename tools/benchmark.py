"""Same local Modbus load for Python and Rust; no production PLC connections."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import httpx
import websockets
import psutil

parser=argparse.ArgumentParser()
parser.add_argument('--source',type=Path,default=Path('D:/PLCLink/FactoryLink'))
parser.add_argument('--devices',type=int,default=50)
parser.add_argument('--points',type=int,default=10)
parser.add_argument('--duration',type=float,default=10)
parser.add_argument('--latency-ms',type=float,default=2)
parser.add_argument('--rust-only',action='store_true')
args=parser.parse_args();root=Path(__file__).resolve().parents[1]

def free_port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]

async def main():
    wire_reads=0
    async def plc(reader,writer):
        nonlocal wire_reads
        try:
            while True:
                h=await reader.readexactly(7);transaction,protocol,length,unit=struct.unpack('>HHHB',h)
                request=await reader.readexactly(length-1);function,address,count=struct.unpack('>BHH',request)
                await asyncio.sleep(args.latency_ms/1000)
                payload=bytes([function,count*2])+b'\x00\x2a'*count
                writer.write(struct.pack('>HHHB',transaction,0,len(payload)+1,unit)+payload);await writer.drain();wire_reads+=1
        except (asyncio.IncompleteReadError,ConnectionError):pass
        finally:writer.close()
    server=await asyncio.start_server(plc,'127.0.0.1',0);plc_port=server.sockets[0].getsockname()[1]
    config={'gateway_name':'benchmark','collect_interval':100,'devices':[{'id':f'd{i}','name':f'PLC-{i}','protocol':'modbus_tcp','ip':'127.0.0.1','port':plc_port,'enabled':True,'points':[{'name':f'p{j}','address':str(40001+j),'type':'uint16','rate':1.0,'offset':0.0,'unit':''} for j in range(args.points)]} for i in range(args.devices)],'mqtt':{'enabled':False},'reconnect':{'max_retries':0,'base_delay':1,'max_delay':60}}
    results=[]
    for implementation in (['rust'] if args.rust_only else ['python','rust']):
        with tempfile.TemporaryDirectory(prefix=f'rustlink-benchmark-{implementation}-') as temp:
            directory=Path(temp);(directory/'config.json').write_text(json.dumps(config),encoding='utf-8');port=free_port()
            executable=root/('target/release/rustlink.exe' if os.name=='nt' else 'target/release/rustlink')
            command=([sys.executable,str(root/'tools/run_original.py'),str(args.source),temp,str(port)] if implementation=='python' else [str(executable),'--headless','--strict-port','--port',str(port),'--data-dir',temp])
            with (directory/'process.log').open('wb') as log:
                process=subprocess.Popen(command,stdout=log,stderr=log,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
                try:
                    async with httpx.AsyncClient(base_url=f'http://127.0.0.1:{port}',timeout=15) as client:
                        for _ in range(100):
                            try:await client.get('/api/health');break
                            except httpx.HTTPError:await asyncio.sleep(.1)
                        else:raise RuntimeError((directory/'process.log').read_text(errors='replace'))
                        count=0;good=0;latencies=[];seen=set();rss=[]
                        def processes():
                            try:
                                parent=psutil.Process(process.pid)
                                return [parent]+parent.children(recursive=True)
                            except psutil.NoSuchProcess:return []
                        def cpu_seconds():
                            return sum(p.cpu_times().user+p.cpu_times().system for p in processes())
                        async with websockets.connect(f'ws://127.0.0.1:{port}/ws/data',max_queue=8192) as ws:
                            async def receive():
                                nonlocal count,good
                                async for raw in ws:
                                    v=json.loads(raw)
                                    if v.get('type')=='data':
                                        count+=1;seen.add(v['device_id']);good+=sum(p['quality']=='good' and p['value']==42 for p in v['values'].values())
                            reader=asyncio.create_task(receive());await client.post('/api/collect/start');await asyncio.sleep(1)
                            count=0;good=0;seen.clear();reads_start=wire_reads;cpu_start=cpu_seconds();begin=time.perf_counter()
                            while time.perf_counter()-begin<args.duration:
                                start=time.perf_counter();response=await client.get('/api/health');response.raise_for_status();latencies.append((time.perf_counter()-start)*1000);rss.append(sum(p.memory_info().rss for p in processes()));await asyncio.sleep(.05)
                            elapsed=time.perf_counter()-begin;reads=wire_reads-reads_start
                            ordered=sorted(latencies)
                            results.append({'implementation':implementation,'seconds':round(elapsed,3),'device_samples':count,'good_point_values':good,'devices_observed':len(seen),'device_samples_per_second':round(count/elapsed,2),'good_points_per_second':round(good/elapsed,2),'wire_reads_per_second':round(reads/elapsed,2),'health_requests':len(latencies),'health_p50_ms':round(statistics.median(latencies),3),'health_p95_ms':round(ordered[int((len(ordered)-1)*.95)],3),'health_max_ms':round(max(latencies),3)})
                            results[-1].update({'peak_rss_mib':round(max(rss)/1024**2,2),'cpu_seconds':round(cpu_seconds()-cpu_start,3)})
                            reader.cancel()
                            try:await reader
                            except asyncio.CancelledError:pass
                    assert results[-1]['good_point_values']>0,'No valid PLC values observed'
                finally:
                    descendants=psutil.Process(process.pid).children(recursive=True) if process.poll() is None else []
                    for child in descendants:
                        try:child.kill()
                        except psutil.NoSuchProcess:pass
                    process.terminate();process.wait(timeout=10)
                    psutil.wait_procs(descendants,timeout=10)
            print(json.dumps(results[-1],ensure_ascii=False),flush=True)
    server.close();await server.wait_closed()
    report={'environment':{'platform':platform.platform(),'python':platform.python_version(),'cpu_count':os.cpu_count()},'load':vars(args)|{'source':str(args.source)},'results':results,'scope':'Local loopback simulation; excludes physical PLC/serial timing and cross-machine HA.'}
    output=root/'artifacts';output.mkdir(exist_ok=True);(output/'benchmark.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
asyncio.run(main())
