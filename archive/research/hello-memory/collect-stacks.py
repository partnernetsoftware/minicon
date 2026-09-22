import subprocess,time,pathlib,os
r=pathlib.Path('target/hello-memory-track')
for mode in ['hello','borderless-hello']:
 env=os.environ.copy();env['MallocStackLogging']='1'
 p=subprocess.Popen([str(r/'probe'),mode],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,env=env)
 try:
  assert p.stdout.readline().strip()=='READY';time.sleep(2)
  s=subprocess.check_output(['malloc_history',str(p.pid),'-allBySize'],text=True,stderr=subprocess.STDOUT)
  (r/(mode+'-stacks.txt')).write_text(s.replace(str(pathlib.Path.home()),'~'))
  print(mode+' done',flush=True)
 finally:p.terminate();p.wait(timeout=10)
