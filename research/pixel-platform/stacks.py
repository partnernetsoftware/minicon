"""Separate instrumented runs: allocation stacks are not RSS budget receipts."""
import os
import pathlib
import subprocess
import time

root = pathlib.Path('target/pixel-platform-stacks')
root.mkdir(parents=True, exist_ok=True)
for name, binary, args in [
    ('native', 'target/hello-pixel-host/compat-regular', ['pixels-input-menu']),
    ('shared', 'target/release/pixel-platform-probe', []),
]:
    env = os.environ.copy()
    env.pop('PROBE_TICK', None)
    env['MallocStackLogging'] = '1'
    with (root / f'{name}.log').open('w') as log:
        p = subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stderr=log, text=True, env=env)
        try:
            assert p.stdout.readline().strip() == 'READY'
            time.sleep(3)
            for tool, args in [('vmmap', ['-w']), ('heap', ['-s']), ('malloc_history', ['-allBySize'])]:
                command = [tool, str(p.pid), *args] if tool == 'malloc_history' else [tool, *args, str(p.pid)]
                r = subprocess.run(command, capture_output=True, text=True, timeout=45)
                if r.returncode: raise RuntimeError(f'{tool} exited {r.returncode}')
                (root / f'{name}-{tool}.txt').write_text(
                    (r.stdout + r.stderr).replace(str(pathlib.Path.home()), '~'))
            print(name, 'captured', flush=True)
        finally:
            p.terminate()
            p.wait(timeout=10)
