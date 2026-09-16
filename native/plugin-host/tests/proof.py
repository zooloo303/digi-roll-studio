#!/usr/bin/env python3
"""Real VST3 boundary proof, one consolidated test runner; no firmware/device."""
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import wave

host, plugin = map(lambda s: str(Path(s).resolve()), sys.argv[1:])
with tempfile.TemporaryDirectory(prefix='drs-host-proof-') as tmp:
    root = Path(tmp)
    def run(name, rate=48000, block=256, level=None, state=None, bad=None, warmup=0):
        out = root / name
        out.mkdir()
        spec = {'path': plugin}
        if level is not None: spec['parameters'] = [{'index': 0, 'value': level}]
        if state: spec['stateIn'] = str(state)
        events = [{'sample': s, 'instance': i, 'channel': ch, 'note': 60, 'velocity': 100}
                  for s, i, ch in [(0, 0, 1), (block-1, 1, 2), (block, 0, 3), (block+17, 1, 6)]]
        cfg = {'rate': rate, 'block': block, 'seconds': .1, 'output': str(out),
               'warmupSeconds': warmup, 'plugins': [spec, spec], 'events': events, 'parallel': bool(os.environ.get('DRS_TEST_PARALLEL'))}
        if bad: cfg.update(bad)
        path = out / 'scenario.json'
        path.write_text(json.dumps(cfg))
        result = subprocess.run([host, str(path)], capture_output=True, text=True, timeout=20)
        if bad:
            assert result.returncode != 0, result.stdout
            return
        assert result.returncode == 0, result.stderr
        report = json.loads((out / 'report.json').read_text())
        assert report['warmupBlocks'] == math.ceil(warmup*rate/block)
        assert report['warmupWallSeconds'] >= warmup
        assert report['eventsDelivered'] == 4
        assert report['frames'] == int(rate*.1)
        assert report['plugins'][0]['parameters'][0]['id']
        with wave.open(str(out / 'mix.wav')) as wav:
            assert wav.getnchannels() == 2 and wav.getsampwidth() == 3
            raw = wav.readframes(wav.getnframes())
        samples = [int.from_bytes(raw[x:x+3], 'little', signed=True)/8388608 for x in range(0,len(raw),6)]
        expected = {e['sample']: .25*(level if level is not None else .25)*e['channel']/16 for e in events}
        for index, sample in enumerate(samples):
            assert abs(sample-expected.get(index, 0)) < 2e-7, (name,index,sample,expected)
        return out
    for rate in (44100,48000):
        for block in (128,256,512): run(f'{rate}-{block}',rate,block)
    os.environ['DRS_TEST_PARALLEL']='1'
    for rate in (44100,48000):
        for block in (128,256,512): run(f'parallel-{rate}-{block}',rate,block)
    run('warmup-parallel',warmup=.05)
    os.environ.pop('DRS_TEST_PARALLEL')
    run('warmup-serial',warmup=.05)
    run('warmup-sub-sample',warmup=.000001)
    saved = run('saved',level=.75)
    run('mutated',level=0)
    # Fresh process and fresh VST3 instance. Expected amplitude is independently .75.
    # Explicitly avoid setting the parameter during recall.
    original = (saved/'scenario.json').read_text()
    cfg = json.loads(original)
    recall = root/'recall'; recall.mkdir()
    cfg['output']=str(recall)
    for spec in cfg['plugins']:
        spec.pop('parameters'); spec['stateIn']=str(saved/'instance-0.state')
    scenario=recall/'scenario.json'; scenario.write_text(json.dumps(cfg))
    result=subprocess.run([host,str(scenario)],capture_output=True,text=True,timeout=20)
    assert result.returncode==0,result.stderr
    assert (recall/'mix.wav').read_bytes()==(saved/'mix.wav').read_bytes()
    # Native-ID parameter changes precede notes on the same block boundary.
    out=root/'parameter-events';out.mkdir()
    native_id=json.loads((saved/'report.json').read_text())['plugins'][0]['parameters'][0]['id']
    assert native_id != '0', 'Must report native VST3 ID, not parameter index'
    cfg={'rate':48000,'block':256,'seconds':.1,'warmupSeconds':.05,'output':str(out),
         'plugins':[{'path':plugin}],
         'events':[{'sample':t,'instance':0,'channel':1,'note':60,'velocity':100} for t in (0,255,256,512)],
         'parameterEvents':[{'sample':256,'instance':0,'id':native_id,'value':0},
                            {'sample':512,'instance':0,'id':native_id,'value':.5}]}
    path=out/'scenario.json';path.write_text(json.dumps(cfg))
    result=subprocess.run([host,str(path)],capture_output=True,text=True,timeout=20)
    assert result.returncode==0,result.stderr
    with wave.open(str(out/'mix.wav')) as wav: raw=wav.readframes(wav.getnframes())
    for frame,expected in [(0,.25/64),(255,.25/64),(256,0),(512,.5/64)]:
        value=int.from_bytes(raw[frame*6:frame*6+3],'little',signed=True)/8388608
        assert abs(value-expected)<2e-7,(frame,value,expected)
    # The fixture reports host seconds/PPQ through the actual VST3 process context.
    out=root/'transport'; out.mkdir()
    cfg={'rate':48000,'block':256,'seconds':.1,'warmupSeconds':.05,'output':str(out),
         'plugins':[{'path':plugin,'parameters':[{'index':1,'value':1}]}],
         'transport':[{'sample':0,'ppq':2,'bpm':120,'playing':False},
                      {'sample':512,'ppq':4,'bpm':60,'playing':True},
                      {'sample':2048,'ppq':8,'bpm':240,'playing':False}]}
    path=out/'scenario.json';path.write_text(json.dumps(cfg))
    result=subprocess.run([host,str(path)],capture_output=True,text=True,timeout=20)
    assert result.returncode==0,result.stderr
    with wave.open(str(out/'mix.wav')) as wav: raw=wav.readframes(wav.getnframes())
    for frame, seconds, ppq in [(0,1,2),(256,1,2),(512,4,4),(768,4+256/48000,4+256/48000),(2048,2,8),(2304,2,8)]:
        for ch,expected in enumerate((seconds,ppq)):
            offset=frame*6+ch*3
            value=int.from_bytes(raw[offset:offset+3],'little',signed=True)/8388608
            assert abs(value-expected/4096)<2e-7,(frame,ch,value,expected/4096)
    run('invalid-warmup',bad={'warmupSeconds':-1})
    run('excessive-warmup',bad={'warmupSeconds':31})
    run('invalid-channel',bad={'events':[{'sample':0,'instance':0,'channel':17,'note':60,'velocity':100}]})
    run('invalid-cc',bad={'events':[{'sample':0,'instance':0,'channel':1,'cc':128,'value':0}]})
    run('invalid-sysex',bad={'events':[{'sample':0,'instance':0,'sysex':[240,127]}]})
    run('missing-plugin',bad={'plugins':[{'path':str(root/'missing.vst3')}]})
print('PASS: twelve serial/concurrent rate/block cases; exact stereo impulses across block boundaries and two instances while stopped; zero parameter; fresh-process state recall; stopped/play/seek/tempo process context; invalid input/missing plugin')
