from pathlib import Path
import subprocess as s,os,time,select,json,hashlib
root=Path.cwd();out=root/'target/frame-lifetime/sample-class-list';env=os.environ.copy();env['EXTERNAL_STAGE_DIRECTORY']=str(out)
binary=out/'probe';err=(out/'host.log').open('w');p=s.Popen([str(binary)],stdin=s.PIPE,stdout=s.PIPE,stderr=err,text=True,env=env)
sampler=None
try:
 if not select.select([p.stdout],[],[],20)[0]:raise RuntimeError('native handshake timeout')
 line=p.stdout.readline();print(line.strip(),flush=True)
 if line.strip()!='BEFORE_FINISH':raise RuntimeError(line)
 sampler=s.Popen(['sample',str(p.pid),'3','1','-file',str(out/'sample.txt')],stdout=s.PIPE,stderr=s.STDOUT,text=True)
 started=False;messages=[]
 until=time.monotonic()+15
 while time.monotonic()<until:
  if select.select([sampler.stdout],[],[],.2)[0]:
   line=sampler.stdout.readline();messages.append(line);print(line.strip(),flush=True)
   if 'Sampling process' in line:started=True;break
   if not line and sampler.poll() is not None:break
 if not started:raise RuntimeError('sample did not acknowledge sampling within 15 seconds: '+''.join(messages))
 p.stdin.write('\n');p.stdin.flush()
 stdout,_=sampler.communicate(timeout=15);messages.append(stdout);(out/'sampler.log').write_text(''.join(messages))
 p.communicate(timeout=15)
 print('sample_return',sampler.returncode,'native_return',p.returncode,flush=True)
finally:
 if sampler and sampler.poll() is None:sampler.kill();sampler.wait()
 if p.poll() is None:p.kill();p.wait()
 err.close()
 (out/'identity.json').write_text(json.dumps({'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pid':p.pid,'native_exit':p.returncode,'sample_exit':sampler.returncode if sampler else None},indent=2))
