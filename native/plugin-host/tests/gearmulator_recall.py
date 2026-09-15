#!/usr/bin/env python3
"""Optional fresh-process recall check for original and post-boot level-zero states.
Does not establish arbitrary kit/user-sample fidelity.
"""
import argparse,json,os,subprocess
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('host',type=Path);p.add_argument('baseline',type=Path)
p.add_argument('mutated',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
base=a.baseline.resolve();mutated=a.mutated.resolve();root=a.output.resolve();root.mkdir(parents=True,exist_ok=False)
results=[]
for name,source in [('mutated',mutated),('original',base)]:
 out=root/name;out.mkdir();cfg=json.loads((base/'scenario.json').read_text())
 cfg.update(output=str(out),seconds=12,editors=False,events=[]);cfg.pop('device',None)
 expected=json.loads((source/'report.json').read_text())
 for i,spec in enumerate(cfg['plugins']):
  spec['stateIn']=str(source/f'instance-{i}.state');spec.pop('parameters',None)
 path=out/'scenario.json';path.write_text(json.dumps(cfg,indent=2))
 with (out/'host.log').open('w') as f:
  r=subprocess.run([str(a.host.resolve()),str(path)],env=dict(os.environ,GEARMULATOR_DATA_ROOT=str(base/'gearmulator')),stdout=f,stderr=subprocess.STDOUT,timeout=120)
 if r.returncode:raise SystemExit(r.returncode)
 actual=json.loads((out/'report.json').read_text())
 values=[]
 for i in range(2):
  exp=next(p['value'] for p in expected['plugins'][i]['parameters'] if p['name']=='Track 1 Level')
  value=next(p['value'] for p in actual['plugins'][i]['parameters'] if p['name']=='Track 1 Level')
  assert abs(exp-value)<1e-6,(name,i,exp,value)
  values.append(value)
 results.append({'case':name,'track1Levels':values})
(root/'recall.json').write_text(json.dumps(results,indent=2));print(json.dumps(results))
