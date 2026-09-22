"""Summarize kernel ledgers without relabeling footprint or vmmap as RSS."""
import json,pathlib,statistics
root=pathlib.Path('archive/research/rss-ledger')
result=[]
for group in ['calibrate','product','native','stages']:
 for row in json.loads((root/(group+'-results.json')).read_text()):
  item={k:row[k] for k in ['variant','mode','repeat','rss_median']}
  item['group']=group
  if row['mode']=='active':
   snapshots=[x['ledger'] for x in row['series']]
   item['median_bytes']={k:statistics.median(x[k] for x in snapshots) for k in ['resident','internal','external','reusable','compressed','phys_footprint']}
   # Component medians need not add up; closure is checked on each exact receipt.
   item['max_receipt_closure_error_bytes']=max(abs(x['resident']-x['internal']-x['external']-x['reusable']) for x in snapshots)
   item['max_ps_vs_latest_ledger_difference_bytes']=max(abs(x['ps_rss']-x['ledger']['resident']) for x in row['series'])
   item['raw_time_offsets_valid']=all(0<=x['sample_age_ms']<1500 for x in row['series'])
  result.append(item)
(root/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
for row in result:
 print(row['group'],row['variant'],row['mode'],row['repeat'],round(row['rss_median']/1048576,3),
       {k:round(v/1048576,3) for k,v in row.get('median_bytes',{}).items()})
