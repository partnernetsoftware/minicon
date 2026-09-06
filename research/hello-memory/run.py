import subprocess,time,json,pathlib,statistics,os
root=pathlib.Path('target/hello-memory-track')
modes=['libc','linked','foundation','app-init','app-loop','app-finish','hidden','borderless','blank','hello','hello-noshadow','hello-nofinish']
results=[]
for mode in modes:
 samples=[]
 for repeat in range(3):
  cmd=[str(root/'libc')] if mode=='libc' else [str(root/'probe'),mode]
  p=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
  try:
   assert p.stdout.readline().strip()=='READY'
   time.sleep(2)
   rss=int(subprocess.check_output(['ps','-o','rss=','-p',str(p.pid)],text=True))*1024
   samples.append(rss)
   if repeat==0:
    out=subprocess.check_output(['vmmap','-resident',str(p.pid)],text=True,stderr=subprocess.STDOUT)
    out=out.replace(str(pathlib.Path.home()),'~')
    (root/(mode+'-vmmap.txt')).write_text(out)
  finally:
   p.terminate();p.wait(timeout=10)
 row={'mode':mode,'rss_bytes':samples,'median_mib':statistics.median(samples)/1048576}
 results.append(row);print(json.dumps(row),flush=True)
 (root/'results.json').write_text(json.dumps(results,indent=2)+'\n')
