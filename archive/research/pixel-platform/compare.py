"""Alternate native, shared-host and terminal processes on the same host."""
import os
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import time

out = pathlib.Path('target/pixel-platform-comparison')
out.mkdir(parents=True, exist_ok=True)
rows = []
variants = [('native', 'target/hello-pixel-host/compat'),
            ('shared', 'target/release/pixel-platform-probe'),
            ('shared-tick', 'target/release/pixel-platform-probe'),
            ('minicon', 'target/release/minicon')]
for repeat in range(3):
    for variant, path in variants:
        binary = pathlib.Path(path).resolve()
        temporary = pathlib.Path(tempfile.mkdtemp(prefix='pixel-compare-', dir='/private/tmp'))
        endpoint = 'unix:' + str(temporary / 'control.sock')
        command = [str(binary)]
        if variant == 'native':
            command += ['pixels-input-menu']
        elif variant == 'minicon':
            command += ['--no-activate', '--control', endpoint, '--cols', '80', '--rows', '24', '-e', '/bin/cat']
        with (out / f'{variant}-{repeat}.log').open('w') as log:
            env = os.environ.copy()
            env.pop('PROBE_TICK', None)
            if variant == 'shared-tick': env['PROBE_TICK'] = '1'
            p = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=log, text=True, env=env)
            try:
                if variant == 'minicon':
                    for _ in range(100):
                        if (temporary / 'control.sock').exists(): break
                        if p.poll() is not None: raise RuntimeError('host exited')
                        time.sleep(.1)
                    else: raise TimeoutError('control socket')
                    subprocess.run([str(binary), 'cli', '--control', endpoint, 'ui-snapshot'],
                                   capture_output=True, check=True, timeout=10)
                else:
                    assert p.stdout.readline().strip() == 'READY'
                time.sleep(3)
                rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                samples = [rss]
                for _ in range(4):
                    time.sleep(1)
                    samples.append(int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024)
                row = dict(variant=variant, repeat=repeat, rss_bytes=rss, rss_series_bytes=samples,
                           sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                rows.append(row)
                print(json.dumps(row), flush=True)
                (out / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
                if repeat == 0:
                    for tool, args in [('vmmap', ['-w']), ('heap', ['-s'])]:
                        result = subprocess.run([tool, *args, str(p.pid)], capture_output=True, text=True, timeout=30)
                        (out / f'{variant}-{tool}.txt').write_text(
                            (result.stdout + result.stderr).replace(str(pathlib.Path.home()), '~'))
            finally:
                if variant == 'minicon':
                    subprocess.run([str(binary), 'cli', '--control', endpoint, 'close-window'],
                                   capture_output=True, timeout=5)
                else:
                    p.terminate()
                try: p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait(timeout=5)
                shutil.rmtree(temporary)
