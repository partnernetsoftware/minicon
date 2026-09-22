"""Prepare bounded sample handshake at finishLaunching; run sample-finish.py next."""
from pathlib import Path
import subprocess as s, os
out=Path('target/frame-lifetime/sample-class-list');out.mkdir(exist_ok=True)
s.run(['python3','archive/research/frame-lifetime/prepare-external-stages.py'],env={**os.environ,'EXTERNAL_STAGE_DETAIL':'1'},check=True)
source=Path('target/frame-lifetime/external-stages.m').read_text()
source=source.replace('[app finishLaunching];','puts("BEFORE_FINISH");fflush(stdout);getchar();\n        [app finishLaunching];\n        puts("FINISH_RETURNED");fflush(stdout);')
source=source.replace('i<100','i<5');(out/'probe.m').write_text(source)
s.run(['clang','-O2','-g','-fobjc-arc','-framework','Cocoa','-framework','QuartzCore','-framework','CoreGraphics','-Wl,-sectcreate,__TEXT,__info_plist,archive/research/hello-memory/compat.plist','archive/research/frame-lifetime/external-pages.c',str(out/'probe.m'),'-o',str(out/'probe')],check=True)
