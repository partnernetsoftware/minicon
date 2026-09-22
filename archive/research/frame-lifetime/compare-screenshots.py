from pathlib import Path
import os,subprocess,json,re
out=Path('target/frame-lifetime');records=[]
artifacts={'baseline':'target/minicon-allocator-probe/baseline','fixed':'target/screenshot-memory/fixed-pre-pin'}
for cycle in range(2):
 for variant,binary in artifacts.items():
  directory=out/f'screenshot-{variant}-{cycle}'
  env=os.environ.copy();env.update(FRAME_TRACE_MODE='0',FRAME_BINARY=binary,FRAME_OUT=str(directory))
  subprocess.run(['python3','archive/research/frame-lifetime/run.py'],env=env,check=True)
  r=json.loads((directory/'results.json').read_text());r.update(variant=variant,cycle=cycle)
  decoded=subprocess.run(['sips','-s','format','tiff',str(directory/'pane.png'),'--out',str(directory/'decoded.tiff')],capture_output=True,text=True,check=True)
  dimensions=subprocess.check_output(['sips','-g','pixelWidth','-g','pixelHeight',str(directory/'decoded.tiff')],text=True)
  r['png']={'size':[int(re.search(r'pixelWidth: (\d+)',dimensions)[1]),int(re.search(r'pixelHeight: (\d+)',dimensions)[1])],'decoded':True}
  r['diagnostics']={}
  for phase in ['restored','screenshot','greeting']:
   vmmap=(directory/(phase+'-vmmap.txt')).read_text();heap=(directory/(phase+'-heap.txt')).read_text()
   r['diagnostics'][phase]={'large_region_lines':[line for line in vmmap.splitlines() if line.startswith('MALLOC_LARGE')], 'heap_live_bytes':int(re.search(r'All zones: \d+ nodes \((\d+) bytes\)',heap)[1])}
  records.append(r);(out/'screenshot-comparison.json').write_text(json.dumps(records,indent=2))
  print('DONE',variant,cycle,r['png'],flush=True)
Path('archive/research/frame-lifetime/screenshot-results.json').write_text(json.dumps(records,indent=2))
