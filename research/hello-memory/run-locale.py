"""Separate +initialize and locale work from NSApplication instance setup."""
import os
import json
import pathlib
import statistics
import subprocess
import time

components = os.environ.get('INIT_COMPONENTS') == '1'
root = pathlib.Path('target/hello-memory-components' if components else 'target/hello-memory-locale')
modes = (['app-class', 'component-appearance', 'component-screen', 'component-workspace',
          'component-pasteboard', 'component-font', 'app-init'] if components else
         ['linked', 'locale-fixed', 'locale-current', 'app-class', 'app-init'])
if os.environ.get('INIT_MENUS') == '1':
    root = pathlib.Path('target/hello-memory-menu-stages')
    modes = ['hello', 'hello-menu-empty', 'hello-menu-class', 'hello-menu-text', 'hello-menu']
root.mkdir(parents=True, exist_ok=True)
subprocess.run(['clang', '-O2', 'research/hello-memory/probe.m', '-framework',
                'Cocoa', '-o', str(root / 'probe')], check=True)
rows = []
for repeat in range(3):
    for mode in modes:
        p = subprocess.Popen([str(root / 'probe'), mode], stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, text=True)
        try:
            assert p.stdout.readline().strip() == 'READY'
            time.sleep(2)
            rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
            row = dict(mode=mode, repeat=repeat, rss_bytes=rss)
            rows.append(row)
            print(json.dumps(row), flush=True)
            (root / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
        finally:
            p.terminate()
            p.wait(timeout=10)
for mode in dict.fromkeys(r['mode'] for r in rows):
    print(mode, statistics.median(r['rss_bytes'] / 1048576 for r in rows if r['mode'] == mode))
