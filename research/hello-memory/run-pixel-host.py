"""Matched capability stages; generated binaries and diagnostics stay in target/."""
import os
import hashlib
import json
import pathlib
import statistics
import subprocess
import time

root = pathlib.Path('target/hello-pixel-host')
root.mkdir(parents=True, exist_ok=True)
policy_only = os.environ.get('PIXEL_POLICY_ONLY') == '1'
designs = ['compat', 'compat-regular'] if policy_only else ['modern', 'compat']
for design in designs:
    policy = ['-DRESEARCH_POLICY=NSApplicationActivationPolicyRegular'] if design == 'compat-regular' else []
    plist_design = 'compat' if design == 'compat-regular' else design
    subprocess.run(['clang', '-O2', '-Wall', '-Wextra', '-fobjc-arc',
                    'research/hello-memory/pixel-host.m', '-framework', 'Cocoa',
                    '-framework', 'QuartzCore', *policy,
                    '-Wl,-sectcreate,__TEXT,__info_plist,research/hello-memory/' + plist_design + '.plist',
                    '-o', str(root / design)], check=True)
rows = []
modes = ['pixels-input-menu'] if policy_only else ['view', 'input', 'pixels', 'pixels-input', 'pixels-menu', 'pixels-input-menu']
for repeat in range(3):
    for mode in modes:
        for design in designs:
            binary = root / design
            with (root / f'{design}-{mode}-{repeat}.log').open('w') as log:
                p = subprocess.Popen([str(binary), mode], stdout=subprocess.PIPE, stderr=log, text=True)
                try:
                    assert p.stdout.readline().strip() == 'READY'
                    time.sleep(3 if policy_only else 2)
                    rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                    row = dict(repeat=repeat, mode=mode, design=design, rss_bytes=rss,
                               sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                    rows.append(row)
                    print(json.dumps(row), flush=True)
                    (root / ('policy-results.json' if policy_only else 'results.json')).write_text(json.dumps(rows, indent=2) + '\n')
                finally:
                    p.terminate()
                    p.wait(timeout=10)
for mode in modes:
    for design in designs:
        print(mode, design, statistics.median(r['rss_bytes'] / 1048576 for r in rows
                                              if r['mode'] == mode and r['design'] == design))
