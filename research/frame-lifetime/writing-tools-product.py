"""Run frozen production artifact through existing public harness; private research ONLY."""
from pathlib import Path
import subprocess as s,os,json,hashlib
root=Path.cwd();out=root/'target/frame-lifetime/writing-tools-product';out.mkdir(exist_ok=True)
lib=out/'observer.dylib';binary=root/'target/screenshot-memory/final-minicon'
s.run(['clang','-x','objective-c','-dynamiclib','-O2','-Wall','-Wextra','-Werror','research/frame-lifetime/writing-tools-product.c','-lobjc','-o',str(lib)],check=True)
# The wrapper injects only a GUI exec. Neither harness nor sibling CLI processes
# receive DYLD_INSERT_LIBRARIES. Constructor removes it before PTY shell spawn.
wrapper=out/'launch.py'
wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\nbinary='+repr(str(binary))+'\nenv=os.environ.copy()\nenv.pop("DYLD_INSERT_LIBRARIES",None)\nif len(sys.argv)>1 and sys.argv[1]!="cli":env["DYLD_INSERT_LIBRARIES"]='+repr(str(lib))+'\nos.execve(binary,[binary]+sys.argv[1:],env)\n');wrapper.chmod(0o700)
harness=root/'target/debug/deps/minicon_control-02ccc1069ddc90b8';results={}
for mode in ['default','private-no']:
    run=out/mode;run.mkdir(exist_ok=True)
    env={**os.environ,'MINICON_TEST_BINARY':str(wrapper),'MINICON_RSS_DIAGNOSTICS_DIR':str(run),'RESEARCH_PRIVATE_NO':'1' if mode=='private-no' else '0'};env.pop('DYLD_INSERT_LIBRARIES',None)
    with (run/'court.log').open('w') as log:
        result=s.run([str(harness),'--test-threads=1','--nocapture'],env=env,stdout=log,stderr=s.STDOUT,timeout=180)
    text=(run/'court.log').read_text();receipts=[json.loads(line.split('MINICON_HOST_RSS_RECEIPT ',1)[1]) for line in text.splitlines() if 'MINICON_HOST_RSS_RECEIPT ' in line]
    vmmap=run/'idle-vmmap.txt'
    results[mode]={'exit_code':result.returncode,'rss_receipts':receipts,'writing_tools_ui_mapped':'WritingToolsUI' in vmmap.read_text() if vmmap.exists() else None,'guard_lines':[line for line in text.splitlines() if 'RESEARCH_PRIVATE_GUARD' in line]}
    print(mode,json.dumps(results[mode]),flush=True)
(out/'results.json').write_text(json.dumps({'artifact_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'observer_sha256':hashlib.sha256(lib.read_bytes()).hexdigest(),'harness_sha256':hashlib.sha256(harness.read_bytes()).hexdigest(),'runs':results},indent=2))
