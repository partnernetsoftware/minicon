import subprocess as s, tempfile, time, json, pathlib, os, hashlib
out=pathlib.Path(os.environ.get('TRACK_OUT','target/minicon-memory-track'));out.mkdir(parents=True,exist_ok=True); binary=pathlib.Path(os.environ.get('TRACK_BINARY','target/release/minicon')).resolve()
root=tempfile.mkdtemp(prefix='minicon-memory-track-',dir='/private/tmp');os.chmod(root,0o700)
endpoint='unix:'+root+'/control.sock'
env=os.environ.copy()
if os.environ.get('TRACK_STACKS'): env['MallocStackLogging']='1'
p=s.Popen([str(binary),'--no-activate','--control',endpoint,'--cols','80','--rows','24','-e','/bin/cat'],stdout=(out/'host.log').open('w'),stderr=s.STDOUT,env=env)
(out/'pid').write_text(str(p.pid))
def cli(*args):
 r=s.run([str(binary),'cli','--control',endpoint,*args],capture_output=True,text=True,timeout=15)
 if r.returncode: raise RuntimeError(r.stderr)
 return r.stdout
def sample(name):
 time.sleep(2)
 values=[int(s.check_output(['ps','-o','rss=','-p',str(p.pid)])) for _ in range(3)]
 for tool,args in [('vmmap',['-resident']),('heap',['-s'])]:
  r=s.run([tool,*args,str(p.pid)],capture_output=True,text=True,timeout=30)
  (out/(name+'-'+tool+'.txt')).write_text((r.stdout+r.stderr).replace(str(pathlib.Path.home()), '~'))
 rec={'phase':name,'rss_kib':values,'tabs':json.loads(cli('list-tabs')),'ui':json.loads(cli('ui-snapshot'))}
 records.append(rec);(out/'results.json').write_text(json.dumps({'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pid':p.pid,'measurements':records},indent=2))
 print(name,values,flush=True)
records=[]
try:
 for _ in range(100):
  if os.path.exists(root+'/control.sock'):break
  time.sleep(.1)
 sample('blank-one-tab')
 if os.environ.get('TRACK_STACKS'):
  print('STACK_WAIT',p.pid,flush=True);time.sleep(55)
 else:
  cli('send-text','ASCII abc 123');sample('ascii')
  cli('send-text','中文漢字');sample('cjk')
  cli('new-tab');sample('two-tabs')
  cli('close-tab','--target','@2');cli('close-tab','--target','@1');sample('greeting')
finally:
 try: cli('close-window')
 except: p.terminate()
 p.wait(timeout=10)
 import shutil
 shutil.rmtree(root)
