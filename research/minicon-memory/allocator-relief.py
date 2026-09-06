"""Same research binary with one delayed allocator-relief call enabled/disabled."""
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import time

root = pathlib.Path('target/minicon-allocator-probe')
binary = (root / 'probe').resolve()
rows = []
for repeat in range(3):
    for trim in [False, True]:
        temporary = pathlib.Path(tempfile.mkdtemp(prefix='minicon-relief-', dir='/private/tmp'))
        endpoint = 'unix:' + str(temporary / 'control.sock')
        env = os.environ.copy()
        env.pop('MINICON_RESEARCH_TRIM', None)
        if trim: env['MINICON_RESEARCH_TRIM'] = '1'
        log_path = root / f'{repeat}-{trim}.log'
        with log_path.open('w') as log:
            p = subprocess.Popen([str(binary), '--no-activate', '--control', endpoint,
                                  '--cols', '80', '--rows', '24', '-e', '/bin/cat'],
                                 stdout=log, stderr=subprocess.STDOUT, env=env)
            def cli(*args):
                return subprocess.run([str(binary), 'cli', '--control', endpoint, *args],
                                      check=True, capture_output=True, text=True, timeout=10)
            try:
                for _ in range(100):
                    if (temporary / 'control.sock').exists(): break
                    if p.poll() is not None: raise RuntimeError('host exited')
                    time.sleep(.1)
                else: raise TimeoutError('control endpoint')
                cli('ui-snapshot')
                samples = []
                start = time.monotonic()
                for _ in range(7):
                    time.sleep(1)
                    rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                    samples.append(dict(seconds=time.monotonic() - start, rss_bytes=rss))
                cli('send-text', 'allocator relief probe\n' * 128)
                time.sleep(2)
                after_input = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                response_start = time.monotonic()
                cli('ui-snapshot')
                response_ms = (time.monotonic() - response_start) * 1000
                match = re.search(r'RESEARCH_TRIM released=(\d+) elapsed_us=(\d+)', log_path.read_text())
                if trim and not match: raise RuntimeError('relief call was not observed')
                row = dict(repeat=repeat, trim=trim, samples=samples, after_input_bytes=after_input,
                           snapshot_ms=response_ms,
                           relief=dict(released=int(match[1]), elapsed_us=int(match[2])) if match else None,
                           sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                rows.append(row)
                print(json.dumps(row), flush=True)
                (root / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
            finally:
                try: cli('close-window')
                except subprocess.SubprocessError: p.terminate()
                try: p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait(timeout=5)
                shutil.rmtree(temporary)
