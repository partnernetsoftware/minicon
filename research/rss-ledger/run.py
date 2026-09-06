"""Sequential owned-child TASK_VM_INFO and external ps RSS measurements."""
import argparse,hashlib,json,os,pathlib,shutil,statistics,subprocess,tempfile,time
root=pathlib.Path('target/rss-ledger')
parser=argparse.ArgumentParser()
parser.add_argument('--group',choices=['calibrate','product','native','stages'],required=True)
parser.add_argument('--repeats',type=int,default=3)
args=parser.parse_args()
# Freeze exact artifacts; never overwrite the canonical product build.
source=pathlib.Path('target/release/minicon')
if args.group=='product':
 frozen=root/'minicon'
 shutil.copy2(source,frozen)
variants={
 'calibrate': [('control',root/'control',[],mode) for mode in ['plain','loaded','active']],
 'product': [('minicon',root/'minicon',[],mode) for mode in ['plain','loaded','active']],
 'native': [('native',pathlib.Path('target/hello-pixel-host/compat-regular'),['pixels-input-menu'],mode) for mode in ['plain','active']],
 'stages': [('native-'+stage,pathlib.Path('target/hello-pixel-host/compat-regular'),[stage],'active') for stage in ['view','input','pixels-input','pixels-input-menu']]
}[args.group]
rows=[]
for repeat in range(args.repeats):
 for variant,binary,extra,mode in variants:
  binary=binary.resolve()
  run=f'{args.group}-{variant}-{mode}-{repeat}'
  temporary=pathlib.Path(tempfile.mkdtemp(prefix='rss-ledger-',dir='/private/tmp'))
  endpoint='unix:'+str(temporary/'control.sock')
  ledger=(root/(run+'.jsonl')).resolve()
  ledger.unlink(missing_ok=True)
  env=os.environ.copy()
  for key in ['DYLD_INSERT_LIBRARIES','RSS_LEDGER_OUTPUT','RSS_LEDGER_EXECUTABLE','PROBE_TICK']:
   env.pop(key,None)
  if mode!='plain':
   env['DYLD_INSERT_LIBRARIES']=str((root/'ledger.dylib').resolve())
   env['RSS_LEDGER_EXECUTABLE']=binary.name
  if mode=='active': env['RSS_LEDGER_OUTPUT']=str(ledger)
  command=[str(binary),*extra]
  if variant=='minicon': command += ['--no-activate','--control',endpoint,'--cols','80','--rows','24','-e','/bin/cat']
  with (root/(run+'.log')).open('w') as log:
   p=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=log,text=True,env=env)
   try:
    if variant=='minicon':
     for _ in range(100):
      if (temporary/'control.sock').exists():break
      if p.poll() is not None:raise RuntimeError('GUI exited')
      time.sleep(.1)
     else:raise TimeoutError('control readiness')
     subprocess.run([str(binary),'cli','--control',endpoint,'ui-snapshot'],capture_output=True,check=True,timeout=10)
    else: assert p.stdout.readline().strip()=='READY'
    time.sleep(3)
    series=[]
    for i in range(5):
     if i:time.sleep(1)
     stamp=time.clock_gettime_ns(time.CLOCK_MONOTONIC)
     rss=int(subprocess.check_output(['ps','-o','rss=','-p',str(p.pid)],text=True))*1024
     record={'ps_monotonic_ns':stamp,'ps_rss':rss}
     if mode=='active':
      samples=[json.loads(line) for line in ledger.read_text().splitlines()]
      assert samples and samples[-1]['pid']==p.pid and samples[-1]['result']==0
      assert samples[-1]['count']>=93
      record['ledger']=samples[-1]
      record['sample_age_ms']=(stamp-samples[-1]['monotonic_ns'])/1e6
     series.append(record)
    row={'clock':'CLOCK_MONOTONIC','variant':variant,'mode':mode,'repeat':repeat,'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'rss_median':statistics.median(s['ps_rss'] for s in series),'series':series}
    rows.append(row)
    (root/(args.group+'-results.json')).write_text(json.dumps(rows,indent=2)+'\n')
    print(json.dumps({k:v for k,v in row.items() if k!='series'}),flush=True)
   finally:
    if variant=='minicon': subprocess.run([str(binary),'cli','--control',endpoint,'close-window'],capture_output=True,timeout=5)
    else:p.terminate()
    try:p.wait(timeout=10)
    except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
    shutil.rmtree(temporary)
# Compact retained evidence; no environment dumps or expanded home paths.
pathlib.Path('research/rss-ledger/'+args.group+'-results.json').write_text(json.dumps(rows,indent=2)+'\n')
