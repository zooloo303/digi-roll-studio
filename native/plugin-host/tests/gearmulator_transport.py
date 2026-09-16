#!/usr/bin/env python3
"""Local real-plugin transport diagnostic; requires baseline states and user ROMs.
No hardware MIDI. Captures are evidence, not a general compatibility certificate.
"""
import argparse, json, math, os, subprocess, wave
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('host',type=Path);p.add_argument('baseline',type=Path);p.add_argument('output',type=Path)
p.add_argument('--model',choices=['MD','MM'],required=True)
p.add_argument('--notes',action='store_true');p.add_argument('--device')
p.add_argument('--pattern',type=int,choices=range(128),metavar='0..127')
p.add_argument('--state',type=Path,help='Restore this snapshot instead of the baseline snapshot')
p.add_argument('--semantics',action='store_true',help='Probe channel, velocity and note-off on part 1')
a=p.parse_args();base=a.baseline.resolve();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
index=0 if a.model=='MD' else 1
original=json.loads((base/'scenario.json').read_text());spec=dict(original['plugins'][index]);spec.pop('parameters',None)
spec['stateIn']=str(a.state.resolve() if a.state else base/f'instance-{index}.state')
rate=48000;block=1024
frame=lambda seconds:round(seconds*rate/block)*block
q10=(frame(10)-frame(4))/rate*2
q22=(frame(22)-frame(16))/rate*1.5
q30=q22+(frame(30)-frame(26))/rate*2.5
changes=[(0,0,120,False),(4,0,120,True),(10,q10,90,True),(16,0,90,True),(22,q22,90,False),(26,q22,150,True),(30,q30,150,False)]
cfg=dict(rate=rate,block=block,seconds=36,warmupSeconds=12,plugins=[spec],output=str(out),editors=False,
         transport=[dict(sample=frame(t),ppq=q,bpm=b,playing=play) for t,q,b,play in changes],events=[])
if a.pattern is not None:
 cfg['events'].append(dict(sample=0,instance=0,sysex=[0,32,60,2 if index==0 else 3,0,113,4,a.pattern]))
if a.device:cfg['device']=a.device
if a.notes:
 for t in [2,6,8,12,14,18,20,24,28,32]:
  cfg['events'] += [dict(sample=frame(t),instance=0,channel=1,note=36 if index==0 else 60,velocity=100),dict(sample=frame(t)+rate//4,instance=0,channel=1,note=36 if index==0 else 60,velocity=0)]
if a.semantics:
 cfg['seconds']=96
 cfg['transport']=[dict(sample=0,ppq=0,bpm=120,playing=False),dict(sample=frame(4),ppq=0,bpm=120,playing=True),dict(sample=frame(88),ppq=168,bpm=120,playing=False)]
 cfg['events']=[e for e in cfg['events'] if 'sysex' in e]
 cases=[(v,1,.25) for v in [1,32,64,100,127]]+[(100,ch,.25) for ch in [2,6,10,16]]+[(100,1,d) for d in [.02,1.5]]+[(0,1,.25)]
 for i,(v,ch,d) in enumerate(cases):
  t=6+i*6
  cfg['events'].append(dict(sample=frame(t),instance=0,channel=ch,note=36 if index==0 else 60,velocity=v))
  if v:cfg['events'].append(dict(sample=frame(t)+round(d*rate),instance=0,channel=ch,note=36 if index==0 else 60,velocity=0))
 cfg['diagnosticCases']=[dict(second=6+i*6,velocity=v,channel=ch,duration=d) for i,(v,ch,d) in enumerate(cases)]
path=out/'scenario.json';path.write_text(json.dumps(cfg,indent=2))
with (out/'host.log').open('w') as log:
 r=subprocess.run([str(a.host.resolve()),str(path)],env=dict(os.environ,GEARMULATOR_DATA_ROOT=str(base/'gearmulator')),stdout=log,stderr=subprocess.STDOUT,timeout=180)
if r.returncode:raise SystemExit(r.returncode)
with wave.open(str(out/'mix.wav')) as w:
 assert w.getsampwidth()==3 and w.getnchannels()==2 and w.getframerate()==rate
 raw=w.readframes(w.getnframes())
windows=[]
for second in range(cfg['seconds']):
 data=raw[second*rate*6:(second+1)*rate*6]
 values=[int.from_bytes(data[i:i+3],'little',signed=True)/8388608 for i in range(0,len(data),3)]
 windows.append(dict(second=second,peak=max(map(abs,values)),rms=math.sqrt(sum(v*v for v in values)/len(values))))
(out/'windows.json').write_text(json.dumps(windows,indent=2))
print(json.dumps(dict(model=a.model,notes=a.notes,activeSeconds=[w['second'] for w in windows if w['rms']>1e-6],peak=max(w['peak'] for w in windows))))
