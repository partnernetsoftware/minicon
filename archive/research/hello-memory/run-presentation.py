"""Test Core Animation action suppression without changing product code."""
import hashlib
import json
import pathlib
import re
import subprocess
import time

root = pathlib.Path('target/pixel-presentation')
root.mkdir(parents=True, exist_ok=True)
binary = root / 'native'
subprocess.run(['clang', '-O2', '-Wall', '-Wextra', '-fobjc-arc',
    '-DRESEARCH_POLICY=NSApplicationActivationPolicyRegular',
    'archive/research/hello-memory/pixel-host.m', '-framework', 'Cocoa', '-framework', 'QuartzCore',
    '-Wl,-sectcreate,__TEXT,__info_plist,archive/research/hello-memory/compat.plist',
    '-o', str(binary)], check=True)
rows = []
for repeat in range(3):
    for action in ['default', 'no-actions', 'null-action']:
        with (root / f'{action}-{repeat}.log').open('w') as log:
            p = subprocess.Popen([str(binary), 'pixels-input-menu-' + action], stdout=subprocess.PIPE,
                                 stderr=log, text=True)
            try:
                assert p.stdout.readline().strip() == 'READY'
                time.sleep(3)
                rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                result = subprocess.run(['vmmap', '-w', str(p.pid)], capture_output=True, text=True, timeout=30)
                mapping = [line for line in result.stdout.splitlines()
                           if line.startswith('VM_ALLOCATE ') and '[ 9008K' in line]
                row = dict(repeat=repeat, action=action, rss_bytes=rss, frame_mapping=mapping,
                           sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                rows.append(row)
                print(json.dumps(row), flush=True)
                (root / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
                (root / f'{action}-{repeat}-vmmap.txt').write_text(
                    (result.stdout + result.stderr).replace(str(pathlib.Path.home()), '~'))
            finally:
                p.terminate()
                p.wait(timeout=10)
