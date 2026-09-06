"""Join self pmap census pages to later same-process vmmap address labels.
Never uses vmmap resident totals as inputs to the census.
"""
import sys,re,json,bisect
from pathlib import Path
census=Path(sys.argv[1]);vmmap=Path(sys.argv[2]);regions=[];runs=[];ledger={};total=None;errors=[]
for line in census.read_text().splitlines():
 fields=line.split('\t')
 if fields[0] in ('ERROR','QUERY_ERROR'):errors.append(fields)
 if fields[0]=='LEDGER':ledger[fields[1]]=dict(zip(['kr','rss','internal','external','reusable','compressed'],map(int,fields[2:])))
 elif fields[0]=='EXTERNAL_RUN':runs.append(tuple(int(s,16) for s in fields[1:3]))
 elif fields[0]=='TOTAL':total=dict(zip(['external_bytes','regions','query_errors','page_size'],map(int,fields[1:])))
if not total:raise SystemExit('Missing complete census footer')
for line in vmmap.read_text().splitlines():
 m=re.match(r'^(.*?)\s+([0-9a-f]+)-([0-9a-f]+)\s+\[.*?\]\s+\S+\s+SM=\S+\s*(.*)$',line)
 if m:regions.append((int(m[2],16),int(m[3],16),m[1].strip(),m[4].strip()))
regions.sort();starts=[x[0] for x in regions];counts={};segments={};page=total['page_size'];seen=set()
for start,end in runs:
 for address in range(start,end,page):
  if address in seen:raise SystemExit('Overlapping page runs: cannot close ledger')
  seen.add(address);index=bisect.bisect_right(starts,address)-1
  matches=[]
  for record in regions[max(0,index-3):index+4]:
   if record[0]<address+page and record[1]>address:matches.append(record)
  labels={(r[2],r[3]) for r in matches}
  if len(labels)==1:
   segment,path=next(iter(labels));label=path or ('<'+segment+'>')
  elif labels:label='<shared-page-boundary-or-alias>'
  else:label='<not-labelled-by-later-vmmap>'
  counts[label]=counts.get(label,0)+page
  segment_types={r[2] for r in matches}
  segment=next(iter(segment_types)) if len(segment_types)==1 else '<mixed-or-unlabelled>'
  segments[segment]=segments.get(segment,0)+page
before=ledger['before']['external'];after=ledger['after']['external'];census_bytes=total['external_bytes']
result={'ledger':ledger,'census':total,'external_scan_drift_bytes':after-before,'closure_to_before_bytes':census_bytes-before,'closure_to_after_bytes':census_bytes-after,'errors':errors,'unique_page_bytes':len(seen)*page,'closure_gate':not errors and ledger['before']['kr']==0 and ledger['after']['kr']==0 and len(seen)*page==census_bytes and total['query_errors']==0 and abs(after-before)<=2*page and min(abs(census_bytes-before),abs(census_bytes-after))<=2*page,'segment_candidate_bytes':dict(sorted(segments.items(),key=lambda item:-item[1])),'labelled_external_pages_bytes':dict(sorted(counts.items(),key=lambda item:-item[1]))}
print(json.dumps(result,indent=2))
