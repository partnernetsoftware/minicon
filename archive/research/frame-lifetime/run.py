from pathlib import Path
import os,tempfile,subprocess as s,time,json,hashlib
root=Path.cwd();tracing=os.environ.get('FRAME_TRACE_MODE','1')=='1';out=root/os.environ.get('FRAME_OUT',('target/frame-lifetime/run-trace' if tracing else 'target/frame-lifetime/run-control'));out.mkdir(parents=True,exist_ok=True)
binary=root/os.environ.get('FRAME_BINARY','target/frame-lifetime/build/release/minicon')
parent=Path(tempfile.mkdtemp(prefix='frame-lifetime-',dir='/private/tmp'));os.chmod(parent,0o700);endpoint='unix:'+str(parent/'control.sock')
env=os.environ.copy();env.pop('MINICON_FRAME_TRACE',None)
if tracing:env['MINICON_FRAME_TRACE']='1'
log=(out/'host.log').open('w');p=s.Popen([str(binary),'--no-activate','--cols','80','--rows','24','--control',endpoint,'-e','/bin/cat'],env=env,stdout=log,stderr=s.STDOUT)
records=[];start=time.monotonic()
def cli(*args):
 r=s.run([str(binary),'cli','--control',endpoint,*args],capture_output=True,text=True,timeout=10)
 if r.returncode:raise RuntimeError((args,r.stdout,r.stderr))
 return r.stdout
def sample(phase,diagnostics=False):
 r={'phase':phase,'elapsed_s':time.monotonic()-start,'rss_kib':int(s.check_output(['ps','-o','rss=','-p',str(p.pid)]))};records.append(r)
 if diagnostics:
  for cmd in [['vmmap','-resident',str(p.pid)],['heap','-s',str(p.pid)]]:
   result=s.run(cmd,capture_output=True,text=True,timeout=30);(out/(phase+'-'+cmd[0]+'.txt')).write_text(result.stdout+result.stderr)
 print(r,flush=True)
try:
 for _ in range(100):
  try:cli('ui-snapshot');break
  except:time.sleep(.1)
 for i in range(6):time.sleep(1);sample('idle-'+str(i),i==5)
 cli('send-text','FRAME_OWNERSHIP_ASCII 中文');time.sleep(2);sample('text',True)
 cli('resize-window','--width','1100','--height','700');time.sleep(2);sample('resized',True)
 cli('resize-window','--width','960','--height','600');time.sleep(2);sample('restored',True)
 cli('screenshot-pane','--output',str(out/'pane.png'));time.sleep(1);sample('screenshot',True)
 cli('close-tab','--target','@1');time.sleep(2);sample('greeting',True)
 cli('close-window');p.wait(timeout=10)
finally:
 if p.poll() is None:p.terminate();p.wait(timeout=10)
 log.close()
 (out/'results.json').write_text(json.dumps({'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pid':p.pid,'exit_code':p.returncode,'records':records},indent=2))
