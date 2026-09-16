#!/usr/bin/env python3
"""MD note-to-part proof using level isolation plus per-note silent controls.
Tests both wrapper automation and direct firmware MIDI CC to isolate routing failures.
Requires a completed MD+MM baseline run and its local firmware data root.
"""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import wave

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('host', type=Path)
p.add_argument('baseline', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--midi-cc', action='store_true', help='Use firmware MIDI CC instead of wrapper parameters')
p.add_argument('--known-kit', action='store_true', help='Assign TRX-BD to every part in this test instance')
a = p.parse_args()
base, out = a.baseline.resolve(), a.output.resolve()
out.mkdir(parents=True, exist_ok=False)
cfg = json.loads((base / 'scenario.json').read_text())
params = {p['name']: p['id'] for p in json.loads((base / 'report.json').read_text())['plugins'][0]['parameters']}
spec = cfg['plugins'][0]
spec['stateIn'] = str(base / 'instance-0.state')
spec.pop('parameters', None)
cfg.update(rate=48000, block=512, seconds=108, plugins=[spec], output=str(out),
           editors=False, parallel=False, events=[], parameterEvents=[], transport=[])
cfg.pop('device', None)
notes = list(range(36,52))
def parameter(time, name, value):
    sample=int(time*48000)//512*512
    if a.midi_cc:
        track=int(name.split()[1])-1
        cc=(12 if name.endswith('Mute') else 8)+track%4
        cfg['events'].append(dict(sample=sample,instance=0,channel=1+track//4,cc=cc,value=round(value*127)))
    else:
        cfg['parameterEvents'].append(dict(sample=sample, instance=0, id=params[name], value=value))
def note(time, pitch):
    for delta, velocity in [(0,100),(.4,0)]:
        cfg['events'].append(dict(sample=int((time+delta)*48000),instance=0,channel=1,note=pitch,velocity=velocity))
for part in range(16):
    parameter(8, f'Track {part+1} Mute', 0)
    if a.known_kit:
        # Elektron OS 1.63 manual Appendix C: SPS-1 TRX-BD, init synth/FX/routing.
        for offset, payload in [(0,[91,part,16,0,2]),(.03,[92,part,6])]:
            cfg['events'].append(dict(sample=int((6+part*.08+offset)*48000),instance=0,sysex=[0,32,60,2,0]+payload))
    start = 10 + part*6
    for other in range(16):
        parameter(start-.25, f'Track {other+1} Level', .8 if part==other else 0)
    note(start, notes[part])
    parameter(start+2.75, f'Track {part+1} Level', 0)
    note(start+3, notes[part])
scenario = out / 'scenario.json'
scenario.write_text(json.dumps(cfg, indent=2))
with (out / 'host.log').open('w') as log:
    result = subprocess.run([str(a.host.resolve()), str(scenario)],
        env=dict(os.environ, GEARMULATOR_DATA_ROOT=str(base/'gearmulator')),
        stdout=log, stderr=subprocess.STDOUT, timeout=180)
if result.returncode:
    raise SystemExit(result.returncode)
with wave.open(str(out/'mix.wav')) as wav:
    assert wav.getframerate()==48000 and wav.getnchannels()==2 and wav.getsampwidth()==3
    raw=wav.readframes(wav.getnframes())
def measure(start):
    data=raw[int(start*48000)*6:int((start+.5)*48000)*6]
    samples=[int.from_bytes(data[i:i+3],'little',signed=True)/8388608 for i in range(0,len(data),3)]
    return dict(peak=max(map(abs,samples)),rms=math.sqrt(sum(v*v for v in samples)/len(samples)))
windows=[]
for part,pitch in enumerate(notes):
    positive,negative=measure(10+part*6),measure(13+part*6)
    windows.append(dict(part=part+1,note=pitch,positive=positive,negative=negative,
                        passed=positive['rms']>1e-4 and negative['rms']<1e-6))
(out/'windows.json').write_text(json.dumps(windows,indent=2))
print(json.dumps(windows,indent=2))
assert all(w['passed'] for w in windows), 'One or more positive/silent controls failed; inspect windows.json'
print('PASS: all 16 MD destinations with level isolation and silent controls')
