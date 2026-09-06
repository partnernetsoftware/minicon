from pathlib import Path
import subprocess as s,os,time,json,hashlib
root=Path.cwd();out=root/'target/frame-lifetime/application-init-dlopen';out.mkdir(exist_ok=True)
lib=out/'observer.dylib';cal=out/'calibrate';binary=out/'probe'
s.run(['clang','-O2','-Wall','-Wextra','-Werror','-dynamiclib','research/frame-lifetime/application-init-dlopen.c','-o',str(lib)],check=True)
s.run(['clang','research/frame-lifetime/application-init-dlopen-calibrate.c','-o',str(cal)],check=True)
env={**os.environ,'DYLD_INSERT_LIBRARIES':str(lib)}
r=s.run([str(cal)],env=env,capture_output=True,text=True,timeout=10);(out/'calibrate.log').write_text(r.stdout+r.stderr)
if r.returncode or 'path=/usr/lib/libSystem.B.dylib success=1' not in r.stderr:raise RuntimeError('calibration failed '+r.stderr)
# Frozen fine-grained stage source: no handshake or private functional changes.
s.run(['clang','-O2','-g','-fobjc-arc','-framework','Cocoa','-framework','QuartzCore','-framework','CoreGraphics','-Wl,-sectcreate,__TEXT,__info_plist,research/hello-memory/compat.plist','research/frame-lifetime/external-pages.c','target/frame-lifetime/external-stages.m','-o',str(binary)],check=True)
env['EXTERNAL_STAGE_DIRECTORY']=str(out)
with (out/'host.log').open('w') as log:
 p=s.Popen([str(binary)],env=env,stdout=log,stderr=s.STDOUT)
 try:
  for _ in range(400):
   text=(out/'host.log').read_text()
   if '\nREADY\n' in text:break
   if p.poll() is not None:raise RuntimeError(text)
   time.sleep(.1)
  else:raise RuntimeError('ready timeout')
  v=s.run(['vmmap','-resident',str(p.pid)],capture_output=True,text=True,timeout=30);(out/'vmmap.txt').write_text(v.stdout+v.stderr)
 finally:
  if p.poll() is None:p.terminate()
  p.wait(timeout=10)
(out/'identity.json').write_text(json.dumps({'artifact_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'observer_sha256':hashlib.sha256(lib.read_bytes()).hexdigest(),'calibration_exit':r.returncode,'native_exit_after_reap':p.returncode},indent=2))
print((out/'host.log').read_text())
