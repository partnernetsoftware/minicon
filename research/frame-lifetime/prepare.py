from pathlib import Path
import subprocess, shutil, tarfile, io
root=Path.cwd(); out=root/'target/frame-lifetime'; out.mkdir(parents=True,exist_ok=True)
product=out/'product'; product.mkdir(exist_ok=True)
archive=subprocess.check_output(['git','archive','ad991b0'])
with tarfile.open(fileobj=io.BytesIO(archive)) as t:t.extractall(product,filter='data')
shared=out/'softbuffer'
shared_archive=subprocess.check_output(['git','-C',str(root/'target/font-platform-fix'),'archive','8d8de88c','third_party/softbuffer'])
with tarfile.open(fileobj=io.BytesIO(shared_archive)) as t:
    for member in t.getmembers():
        prefix='third_party/softbuffer/'
        if member.name.startswith(prefix):
            member.name=member.name[len(prefix):]
            if member.name:t.extract(member,shared,filter='data')
p=product/'Cargo.toml';s=p.read_text();s=s.replace('softbuffer = { git = "https://github.com/partnernetsoftware/agenterm", rev = "8d8de88c9ab62709327a6358609e9a09e8c363fd" }', 'softbuffer = { path = "../softbuffer" }');p.write_text(s)
print('Isolated product and softbuffer ready')
