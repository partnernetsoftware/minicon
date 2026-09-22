from pathlib import Path
import os,subprocess as s,time,json,hashlib
root=Path.cwd();out=root/os.environ.get('EXTERNAL_STAGE_RUN_OUT','target/frame-lifetime/external-stages-run');out.mkdir(parents=True,exist_ok=True)
for p in out.glob('*.tsv'):p.unlink()
env=os.environ.copy();env['EXTERNAL_STAGE_DIRECTORY']=str(out)
if os.environ.get('CLASS_TRACE_LIBRARY'):
 env['DYLD_INSERT_LIBRARIES']=str(root/os.environ['CLASS_TRACE_LIBRARY']);env['CLASS_TRACE_TARGET']='external-stages'
binary=root/'target/frame-lifetime/external-stages';log=(out/'host.log').open('w');p=s.Popen([str(binary)],env=env,stdout=log,stderr=s.STDOUT)
try:
 for _ in range(400):
  text=(out/'host.log').read_text()
  if '\nREADY\n' in text:break
  if p.poll() is not None:raise RuntimeError(text)
  time.sleep(.1)
 else:raise RuntimeError('stage timeout')
 r=s.run(['vmmap','-resident',str(p.pid)],capture_output=True,text=True,timeout=30);(out/'final-vmmap.txt').write_text(r.stdout+r.stderr)
 results={};sets={}
 names=['observer-first','observer-warm','shared-application']
 names+=['regular-policy','window-init','content-view','first-responder','input-context','pixels-present','order-front','finish-launching','event-settle'] if os.environ.get('EXTERNAL_STAGE_DETAIL') else ['window-pixels-input']
 names+=['complete-menu']
 for name in names:
  result=s.check_output(['python3','archive/research/frame-lifetime/attribute-external.py',str(out/(name+'.tsv')),str(out/'final-vmmap.txt')],text=True);results[name]=json.loads(result)
  pages=set();page=results[name]['census']['page_size']
  for line in (out/(name+'.tsv')).read_text().splitlines():
   if line.startswith('EXTERNAL_RUN\t'):
    _,a,b=line.split('\t');pages.update(range(int(a,16),int(b,16),page))
  sets[name]=pages
 previous=None
 for name,result in results.items():
  if previous:
   new=sets[name]-sets[previous];gone=sets[previous]-sets[name];result['change']={'from':previous,'new_bytes':len(new)*page,'gone_bytes':len(gone)*page,'ledger_external_delta':result['ledger']['after']['external']-results[previous]['ledger']['after']['external']}
   lines=['LEDGER\tbefore\t0\t0\t0\t0\t0\t0','LEDGER\tafter\t0\t0\t0\t0\t0\t0']
   lines += [f'EXTERNAL_RUN\t{address:x}\t{address+page:x}' for address in sorted(new)]
   lines += [f'TOTAL\t{len(new)*page}\t0\t0\t{page}']
   newfile=out/(name+'-new.tsv');newfile.write_text('\n'.join(lines)+'\n')
   labels=json.loads(s.check_output(['python3','archive/research/frame-lifetime/attribute-external.py',str(newfile),str(out/'final-vmmap.txt')],text=True))
   result['change']['new_segment_candidates']=labels['segment_candidate_bytes'];result['change']['new_mapping_candidates']=labels['labelled_external_pages_bytes']
  previous=name
 final={'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pid':p.pid,'stages':results};(out/'results.json').write_text(json.dumps(final,indent=2))
 for name,r in results.items():print(name,'external',r['ledger']['after']['external']/1048576,'census',r['census']['external_bytes']/1048576,'gate',r['closure_gate'],'change',r.get('change',{}).get('new_segment_candidates'),flush=True)
finally:
 if p.poll() is None:p.terminate()
 p.wait(timeout=10);log.close()
