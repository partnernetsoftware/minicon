"""Compare frozen artifacts; no Cargo overrides or product mutation here."""
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import time

root = pathlib.Path('target/minicon-menu-probe')
rows = []
for repeat in range(3):
    for variant in ['baseline', 'no-separators']:
        binary = (root / variant).resolve()
        temporary = pathlib.Path(tempfile.mkdtemp(prefix='minicon-menu-', dir='/private/tmp'))
        endpoint = 'unix:' + str(temporary / 'control.sock')
        with (root / f'{variant}-{repeat}.log').open('w') as log:
            p = subprocess.Popen([str(binary), '--no-activate', '--control', endpoint,
                                  '--cols', '80', '--rows', '24', '-e', '/bin/cat'],
                                 stdout=log, stderr=subprocess.STDOUT)
            try:
                for _ in range(100):
                    if (temporary / 'control.sock').exists():
                        break
                    if p.poll() is not None:
                        raise RuntimeError('host exited before readiness')
                    time.sleep(.1)
                else:
                    raise TimeoutError('control endpoint did not appear')
                subprocess.run([str(binary), 'cli', '--control', endpoint, 'ui-snapshot'],
                               check=True, capture_output=True, timeout=10)
                time.sleep(3)
                rss = int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(p.pid)])) * 1024
                row = dict(variant=variant, repeat=repeat, rss_bytes=rss,
                           sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
                rows.append(row)
                print(json.dumps(row), flush=True)
                (root / 'comparison.json').write_text(json.dumps(rows, indent=2) + '\n')
            finally:
                try:
                    subprocess.run([str(binary), 'cli', '--control', endpoint, 'close-window'],
                                   check=True, capture_output=True, timeout=5)
                except (subprocess.SubprocessError, OSError):
                    p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait(timeout=5)
                shutil.rmtree(temporary)
