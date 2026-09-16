#!/usr/bin/env python3
"""Local P1 runner: symlink user ROMs into isolated Gearmulator storage; never package.
Usage: run-gearmulator.py HOST PLUGIN_DIRECTORY MD_ROM MM_ROM OUTPUT [--editors] [--device NAME]
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('host',type=Path); p.add_argument('plugins',type=Path)
p.add_argument('md_rom',type=Path); p.add_argument('mm_rom',type=Path); p.add_argument('output',type=Path)
p.add_argument('--warmup-seconds',type=float,default=12,help='Silent preparation before opening audio (0..30 seconds)')
p.add_argument('--parallel',action='store_true',help='Process independent instances concurrently');
p.add_argument('--editors',action='store_true'); p.add_argument('--device')
p.add_argument('--seconds',type=float,default=30); p.add_argument('--rate',type=int,default=48000); p.add_argument('--block',type=int,default=256)
a=p.parse_args(); out=a.output.resolve(); out.mkdir(parents=True,exist_ok=False)
for model,rom in [('Machinedrum',a.md_rom),('Monomachine',a.mm_rom)]:
    rom=rom.resolve(strict=True)
    if rom.stat().st_size!=8388608: raise SystemExit('Expected 8 MiB user-supplied ROM')
    folder=out/'gearmulator'/'Gearmulator Preview'/model/'roms'; folder.mkdir(parents=True)
    (folder/rom.name).symlink_to(rom)
# Gearmulator MD intercepts notes 36..51 as consecutive panel pads.
# This differs from the physical Machinedrum firmware MIDI map.
notes=list(range(36,52))
events=[]
for i,note in enumerate(notes):
    sample=int((10+i*.5)*a.rate)
    events.extend([dict(sample=sample,instance=0,channel=1,note=note,velocity=100),
                   dict(sample=sample+int(.1*a.rate),instance=0,channel=1,note=note,velocity=0)])
for ch in range(1,7):
    sample=int((19+(ch-1))*a.rate)
    events.extend([dict(sample=sample,instance=1,channel=ch,note=60,velocity=100),
                   dict(sample=sample+int(.4*a.rate),instance=1,channel=ch,note=60,velocity=0)])
cfg=dict(warmupSeconds=a.warmup_seconds,parallel=a.parallel,rate=a.rate,block=a.block,seconds=a.seconds,output=str(out),editors=a.editors,
         plugins=[{'path':str((a.plugins/f'Gearmulator {model}.vst3').resolve())} for model in ['MD','MM']],
         events=[e for e in events if e['sample']<a.seconds*a.rate])
if a.device: cfg['device']=a.device
scenario=out/'scenario.json'; scenario.write_text(json.dumps(cfg,indent=2))
env=dict(os.environ,GEARMULATOR_DATA_ROOT=str(out/'gearmulator'))
with (out/'host.log').open('w') as log:
    result=subprocess.run([str(a.host.resolve()),str(scenario)],env=env,stdout=log,stderr=subprocess.STDOUT,timeout=180)
raise SystemExit(result.returncode)
