"""One bounded read-only sample of native sharedApplication initialization."""
from pathlib import Path
import subprocess as s,os,time,select,json,hashlib
root=Path.cwd();out=root/'target/frame-lifetime/application-init';out.mkdir(exist_ok=True)
# Use already generated, fine-grained frozen native stage source; do not mutate it.
source=(root/'target/frame-lifetime/external-stages.m').read_text()
source=source.replace('NSApplication *app=NSApplication.sharedApplication;', 'puts("BEFORE_APPLICATION");fflush(stdout);getchar();\n        NSApplication *app=NSApplication.sharedApplication;\n        puts("APPLICATION_RETURNED");fflush(stdout);')
source=source.replace('i<100','i<5');(out/'probe.m').write_text(source)
binary=out/'probe'
s.run(['clang','-O2','-g','-fobjc-arc','-framework','Cocoa','-framework','QuartzCore','-framework','CoreGraphics','-Wl,-sectcreate,__TEXT,__info_plist,archive/research/hello-memory/compat.plist','archive/research/frame-lifetime/external-pages.c',str(out/'probe.m'),'-o',str(binary)],check=True)
env=os.environ.copy();env.pop('DYLD_INSERT_LIBRARIES',None);env['EXTERNAL_STAGE_DIRECTORY']=str(out)
err=(out/'host.log').open('w');p=s.Popen([str(binary)],stdin=s.PIPE,stdout=s.PIPE,stderr=err,text=True,env=env);sampler=None
try:
 if not select.select([p.stdout],[],[],20)[0]:raise RuntimeError('native handshake timeout')
 line=p.stdout.readline();print(line.strip(),flush=True)
 if line.strip()!='BEFORE_APPLICATION':raise RuntimeError(line)
 sampler=s.Popen(['sample',str(p.pid),'3','1','-file',str(out/'sample.txt')],stdout=s.PIPE,stderr=s.STDOUT,text=True)
 started=False;messages=[];until=time.monotonic()+15
 while time.monotonic()<until:
  if select.select([sampler.stdout],[],[],.2)[0]:
   line=sampler.stdout.readline();messages.append(line)
   if 'Sampling process' in line:started=True;break
   if not line and sampler.poll() is not None:break
 if not started:raise RuntimeError('sample acknowledgement timeout: '+''.join(messages))
 p.stdin.write('\n');p.stdin.flush()
 stdout,_=sampler.communicate(timeout=15);messages.append(stdout);(out/'sampler.log').write_text(''.join(messages))
 stdout,_=p.communicate(timeout=15);(out/'native-stdout.txt').write_text(stdout)
 print('sample_return',sampler.returncode,'native_return',p.returncode,flush=True)
finally:
 if sampler and sampler.poll() is None:sampler.kill();sampler.wait()
 if p.poll() is None:p.kill();p.wait()
 err.close()
 (out/'identity.json').write_text(json.dumps({'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pid':p.pid,'native_exit':p.returncode,'sample_exit':sampler.returncode if sampler else None},indent=2))
