"""Windows LAN latency client for wire-pgo-20260912.md; CLI: RATE SECONDS."""
import ctypes
import json
import select
import socket
import statistics
import struct
import sys
import time

def packet(query_id, domain):
    name=f'host{domain:03}'.encode()
    return struct.pack('!6H',query_id,0x100,1,0,0,0)+bytes([len(name)])+name+b'\x05bench\0\0\1\0\1'

def run(rate, seconds, port=15353):
    # This changes only the Windows load generator's timer resolution.
    ctypes.windll.winmm.timeBeginPeriod(1)
    sock=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    # Keep the RSS flow hash stable across variants and affinity choices.
    sock.bind(('192.168.1.157',15356))
    sock.connect(('192.168.1.1',port))
    sock.setblocking(False)
    pending={};latencies=[];errors=0;timeouts=0;sent=0
    start=next_send=time.perf_counter();end=start+seconds
    try:
        while time.perf_counter()<end or pending:
            now=time.perf_counter()
            if now<end and now>=next_send and len(pending)<4:
                query_id=sent%65536
                query=packet(query_id,sent%256)
                sock.send(query);pending[query_id]=(time.perf_counter(),query)
                sent+=1
                next_send=max(next_send+1/rate,now)
            for query_id,(began,_) in list(pending.items()):
                if now-began>1:
                    del pending[query_id];timeouts+=1
            wait=max(0,min(next_send-now,0.01)) if now<end and len(pending)<4 else 0.01
            if select.select([sock],[],[],wait)[0]:
                reply=sock.recv(65535);finished=time.perf_counter()
                query_id=struct.unpack_from('!H',reply)[0]
                if query_id not in pending:
                    errors+=1;continue
                began,query=pending.pop(query_id)
                if len(reply)<len(query)+16 or not reply[2]&128 or reply[3]&15 or reply[12:len(query)]!=query[12:] or reply[-4:]!=bytes([192,0,2,1]):
                    errors+=1
                else:latencies.append((finished-began)*1e6)
    finally:
        sock.close();ctypes.windll.winmm.timeEndPeriod(1)
    elapsed=time.perf_counter()-start
    latencies.sort()
    def quantile(p):return round(latencies[min(len(latencies)-1,int(len(latencies)*p))],1) if latencies else None
    return dict(target_qps=rate,seconds=round(elapsed,3),qps=round(len(latencies)/elapsed,1),sent=sent,success=len(latencies),errors=errors,timeouts=timeouts,p50_us=quantile(.5),p95_us=quantile(.95),p99_us=quantile(.99),avg_us=round(statistics.mean(latencies),1) if latencies else None)

if __name__=='__main__':print(json.dumps(run(int(sys.argv[1]),int(sys.argv[2]))))
