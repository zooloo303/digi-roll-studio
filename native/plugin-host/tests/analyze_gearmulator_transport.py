#!/usr/bin/env python3
"""Summarize captured transport probes without inferring musical intent from RMS."""
import argparse,json,math,wave
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('run',type=Path)
g=p.add_mutually_exclusive_group()
g.add_argument('--expect-silence',action='store_true')
g.add_argument('--expect-authored',action='store_true',help='Assert positive note responses and quiet late gaps for these sparse probes')
a=p.parse_args()
cfg=json.loads((a.run/'scenario.json').read_text());report=json.loads((a.run/'report.json').read_text())
if len(cfg['plugins']) != 1:
 raise SystemExit('Per-note analysis requires a single-instrument capture; mixed audio cannot identify each instance.')
with wave.open(str(a.run/'mix.wav')) as w:
 rate=w.getframerate();assert w.getnchannels()==2 and w.getsampwidth()==3
 raw=w.readframes(w.getnframes());frames=len(raw)//6

def measure(start,end):
 lo=max(0,round(start));hi=min(frames,round(end));assert hi>lo
 data=raw[lo*6:hi*6];values=[int.from_bytes(data[i:i+3],'little',signed=True)/8388608 for i in range(0,len(data),3)]
 return dict(peak=max(map(abs,values)),rms=math.sqrt(sum(v*v for v in values)/len(values)))
notes=[e for e in cfg['events'] if e.get('velocity',0)>0]
results=[]
for i,e in enumerate(notes):
 start=e['sample'];end=notes[i+1]['sample'] if i+1<len(notes) else frames
 result=dict(sample=start,channel=e['channel'],velocity=e['velocity'],note=e['note'],response=measure(start,min(end,start+rate)),lateGap=measure(max(start,end-rate*.35),end))
 # First signal above an explicit amplitude threshold, relative to scheduled input.
 # This measures rendered onset only, not acoustic latency or sample-accurate MIDI delivery.
 onset=None
 for f in range(start,min(end,start+rate)):
  if any(abs(int.from_bytes(raw[f*6+c*3:f*6+c*3+3],'little',signed=True)/8388608)>1e-5 for c in range(2)):
   onset=(f-start)/rate*1000;break
 result['renderedOnsetMs']=onset;results.append(result)
summary=dict(notes=results,wholeCapture=measure(0,frames),eventsDelivered=report['eventsDelivered'],blocksOverBudget=report.get('blocksOverBudget'),deviceXruns=report.get('deviceXruns'),device=report.get('device'),interpretation='Response/gap amplitudes and first threshold crossing only; no blanket no-doubling or latency certification.')
if a.expect_silence:
 summary['passed']=summary['wholeCapture']['peak']<1e-6
elif a.expect_authored:
 summary['passed']=bool(results) and all(n['response']['rms']>1e-4 and n['lateGap']['rms']<1e-6 for n in results)
(a.run/'analysis.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))

if summary.get('passed') is False:raise SystemExit('Expected signal/silence criteria failed; inspect analysis.json')
