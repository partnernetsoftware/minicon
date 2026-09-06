from pathlib import Path
import json,collections
root=Path('target/frame-lifetime/run-trace');frames=collections.defaultdict(list)
for line in (root/'host.log').read_text().splitlines():
 if not line.startswith('FRAME '):continue
 fields=dict(x.split('=',1) for x in line.split()[1:])
 for k in list(fields):
  if k not in ('event','ptr'):fields[k]=int(fields[k])
 frames[fields['id']].append(fields)
summary=[]
for id,events in frames.items():
 start=events[0]['t_us'];end=events[-1]['t_us'];released=any(x['event']=='release' for x in events)
 phase_events=[{k:x[k] for k in ('t_us','event','resident','paged_out','copied','modified','live')} for x in events if x['event']!='sample']
 samples=[x for x in events if x['event']=='sample']
 summary.append({'id':id,'bytes':events[0]['bytes'],'released':released,'lifetime_us':end-start,'sample_count':len(samples),'sample_resident_min':min((x['resident'] for x in samples),default=None),'sample_resident_max':max((x['resident'] for x in samples),default=None),'sample_paged_out_max':max((x['paged_out'] for x in samples),default=None),'events':phase_events})
report={'frames':summary,'max_live':max(x['live'] for events in frames.values() for x in events),'unreleased':[x['id'] for x in summary if not x['released']]}
(root/'frame-summary.json').write_text(json.dumps(report,indent=2))
print(json.dumps({'frames':len(frames),'max_live':report['max_live'],'unreleased':report['unreleased'],'resident_min':min((x['resident'] for events in frames.values() for x in events if x['event']=='sample'),default=None),'paged_out_max':max((x['paged_out'] for events in frames.values() for x in events if x['event']=='sample'),default=None)},indent=2))
for f in summary[:5]:print(json.dumps(f))
