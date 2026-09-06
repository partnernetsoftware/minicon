"""No windows or preference changes; child lifetime is bounded by stdin/cleanup."""
import json
import os
import pathlib
import statistics
import subprocess
import time

root = pathlib.Path('target/hello-memory-init')
root.mkdir(parents=True, exist_ok=True)
subprocess.run(['clang', '-O2', 'research/hello-memory/init-stages.m',
                '-framework', 'Cocoa', '-o', str(root / 'probe')], check=True)
rows = []
for repeat in range(3):
    for variant in ['retain', 'drain']:
        p = subprocess.Popen([str(root / 'probe'), variant], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        try:
            for _ in range(6):
                stage = p.stdout.readline().strip()
                assert stage, 'probe exited before completing initialization'
                time.sleep(2)
                rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                row = dict(variant=variant, repeat=repeat, stage=stage, rss_bytes=rss)
                rows.append(row)
                print(json.dumps(row), flush=True)
                (root / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
                if repeat == 0:
                    for tool, args in [('vmmap', ['-w']), ('heap', ['-s'])]:
                        result = subprocess.run([tool, *args, str(p.pid)], capture_output=True,
                                                text=True, timeout=30)
                        (root / f'{variant}-{stage}-{tool}.txt').write_text(
                            (result.stdout + result.stderr).replace(str(pathlib.Path.home()), '~'))
                p.stdin.write('\n')
                p.stdin.flush()
            p.wait(timeout=10)
        finally:
            if p.poll() is None:
                p.terminate()
                p.wait(timeout=10)
for variant in ['retain', 'drain']:
    for stage in dict.fromkeys(r['stage'] for r in rows):
        values = [r['rss_bytes'] / 1048576 for r in rows
                  if r['variant'] == variant and r['stage'] == stage]
        print(variant, stage, statistics.median(values), flush=True)
