#!/usr/bin/env python3
"""Optional real-plugin routing probe. Requires a completed run directory with states.
Solo each expected part with wrapper mute parameters, then address its candidate
Gearmulator MIDI destination. A nonzero window is evidence, not an onset/latency measurement.
"""
import argparse,json,os,subprocess,wave,math
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('host',type=Path);p.add_argument('baseline',type=Path);p.add_argument('output',type=Path)
p.add_argument('--model',choices=['MD','MM'],required=True);a=p.parse_args()
base=a.baseline.resolve();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
index=0 if a.model=='MD' else 1
cfg=json.loads((base/'scenario.json').read_text());report=json.loads((base/'report.json').read_text())
parameters={p['name']:p['id'] for p in report['plugins'][index]['parameters']}
spec=cfg['plugins'][index];spec['stateIn']=str(base/f'instance-{index}.state')
cfg.update(plugins=[spec],editors=False,output=str(out),seconds=60,events=[],parameterEvents=[]);cfg.pop('device',None)
count=16 if a.model=='MD' else 6
# Gearmulator intercepts this chromatic range as consecutive panel pads.
notes=list(range(36,52))
for part in range(count):
    start=10+part*3;sample=int(start*cfg['rate']);control=(sample//cfg['block']-2)*cfg['block']
    for other in range(count):
        cfg['parameterEvents'].append(dict(sample=control,instance=0,id=parameters[f'Track {other+1} Mute'],value=int(other!=part)))
    note=notes[part] if a.model=='MD' else 60;channel=1 if a.model=='MD' else part+1
    cfg['events'].extend([dict(sample=sample,instance=0,channel=channel,note=note,velocity=100),
                          dict(sample=sample+cfg['rate'],instance=0,channel=channel,note=note,velocity=0)])
path=out/'scenario.json';path.write_text(json.dumps(cfg,indent=2))
with (out/'host.log').open('w') as f:
    result=subprocess.run([str(a.host.resolve()),str(path)],env=dict(os.environ,GEARMULATOR_DATA_ROOT=str(base/'gearmulator')),stdout=f,stderr=subprocess.STDOUT,timeout=180)
if result.returncode:raise SystemExit(result.returncode)
with wave.open(str(out/'mix.wav')) as w:raw=w.readframes(w.getnframes());rate=w.getframerate()
windows=[]
for part in range(count):
    start=10+part*3;buf=raw[start*rate*6:(start+1)*rate*6]
    values=[int.from_bytes(buf[i:i+3],'little',signed=True)/8388608 for i in range(0,len(buf),3)]
    windows.append(dict(part=part+1,channel=1 if a.model=='MD' else part+1,note=notes[part] if a.model=='MD' else 60,
                        peak=max(map(abs,values)),rms=math.sqrt(sum(x*x for x in values)/len(values))))
(out/'windows.json').write_text(json.dumps(windows,indent=2));print(json.dumps(windows))
