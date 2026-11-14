#!/usr/bin/env python3
"""Run minimal native-init recovery fixtures; never claim a full distro matrix."""
from pathlib import Path
import concurrent.futures, datetime, hashlib, json, os, re, shutil, subprocess, sys, tempfile
ROOT=Path(__file__).resolve().parents[2]
CACHE=Path(os.environ.get('DMITBOX_DEV_VM_CACHE',ROOT/'.cache/vm-validation')).resolve()
TOOLS=CACHE/'tools'
ARTIFACT=ROOT/'dist/dmitbox-local.sh'
BUSYBOX=Path(os.environ.get('DMITBOX_DEV_STATIC_BUSYBOX',str(shutil.which('busybox') or '')))
ALPINE=Path(os.environ.get('DMITBOX_DEV_ALPINE_ROOT',str(CACHE/'alpine-root')))
VERSION='6.12.107+deb13-amd64'
QEMU=TOOLS/'usr/bin/qemu-system-x86_64'
KERNEL=TOOLS/'boot'/('vmlinuz-'+VERSION)
MKFS=TOOLS/'usr/sbin/mke2fs'
BIOS=TOOLS/'usr/share/seabios/bios-256k.bin'
inputs=[ARTIFACT,BUSYBOX,ALPINE/'bin/bash',ALPINE/'sbin/openrc',QEMU,KERNEL,MKFS,BIOS]
missing=[str(p) for p in inputs if not p.is_file()]
if missing:
    print('SKIP minimal guest fixtures; prepare explicit native inputs (no implicit download): '+', '.join(missing))
    sys.exit(77)
ROOT.joinpath('artifacts').mkdir(exist_ok=True)
report=Path(tempfile.mkdtemp(prefix='guest-fixtures-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-',dir=ROOT/'artifacts'))
env=os.environ.copy();env['LD_LIBRARY_PATH']=str(TOOLS/'usr/lib/x86_64-linux-gnu')
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
artifact_hash=digest(ARTIFACT)
manifest={'schema':1,'classification':'minimal native systemd and OpenRC fixtures; not full distribution images','acceleration':'QEMU TCG, no KVM claim','network':'no guest NIC; serial console only','artifact_sha256':artifact_hash,'inputs':{str(p):digest(p) for p in inputs if p.is_file()},'limitations':['No full Debian/Arch/Alpine manager matrix','No external SSH or IPv6 reachability proof from this fixture','No TCP/BBR performance or kernel Swap validation']}
(report/'inputs.json').write_text(json.dumps(manifest,indent=2)+'\n')
# Lock fixture builds and images so concurrent test runners do not share disks.
import fcntl
lock=(CACHE/'guest-fixtures.lock').open('a');fcntl.flock(lock,fcntl.LOCK_EX)
with (report/'build.log').open('w') as log:
    result=subprocess.run([sys.executable,str(ROOT/'tests/integration/build-guests.py'),'--cache',str(CACHE),'--artifact',str(ARTIFACT),'--busybox',str(BUSYBOX),'--alpine-root',str(ALPINE),'--kernel-version',VERSION],stdout=log,stderr=subprocess.STDOUT)
if result.returncode:
    print('FAIL guest fixture build; evidence '+str(report));sys.exit(1)
manifest['fixture_inputs']={}
for backend,dirname in [('systemd','systemd-root'),('openrc','openrc-root')]:
    fixture_root=CACHE/dirname
    copied_artifact=fixture_root/'opt/dmitbox-local.sh'
    manifest['fixture_inputs'][backend]={
        'initramfs_sha256':digest(CACHE/f'{backend}-initramfs.cpio.gz'),
        'copied_artifact_sha256':digest(copied_artifact),
        'copied_source_sha256':{str(p.relative_to(fixture_root/'opt')):digest(p) for p in sorted((fixture_root/'opt/src').rglob('*.sh'))},
    }
    if digest(copied_artifact)!=artifact_hash:
        print('FAIL artifact changed while building '+backend+'; evidence '+str(report));sys.exit(1)
def boot(backend,disk,phase):
    log=report/f'{backend}-boot{phase}.log'
    args=[str(QEMU),'-L',str(TOOLS/'usr/share/qemu'),'-bios',str(BIOS),'-vga','none','-accel','tcg,thread=single','-m','512','-smp','1','-nographic','-monitor','none','-no-reboot','-nic','none','-kernel',str(KERNEL),'-initrd',str(CACHE/f'{backend}-initramfs.cpio.gz'),'-append','console=ttyS0 rdinit=/init panic=5 quiet','-drive',f'file={disk},format=raw,if=virtio']
    rc=0
    with log.open('w') as f:
        try:rc=subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,stdin=subprocess.DEVNULL,env=env,timeout=180).returncode
        except subprocess.TimeoutExpired:rc=124
    text=log.read_text(errors='replace')
    # One-use disposable fixture challenges must not be retained in public logs.
    log.write_text(re.sub(r'(One-use confirmation challenge \(keep private\):) [a-f0-9]{64}',r'\1 [REDACTED]',text))
    if rc:raise RuntimeError(f'{backend} boot{phase}: QEMU exit {rc}')
    expected=['DMITBOX_VM_ENOSPC_PASS','DMITBOX_VM_READONLY_PASS','APPLY_RC=0 resource=after'] if phase==1 else [f'DMITBOX_{backend.upper()}_REBOOT_PASS','AFTER_REBOOT state=ROLLED_BACK resource=before',f'DMITBOX_{backend.upper()}_DEADLINE_PASS','DMITBOX_VM_CLOCK_CHANGE_PASS']
    if any(marker not in text for marker in expected):raise RuntimeError(f'{backend} boot{phase}: required postcondition marker absent')
    # A hard reset can discard the final userspace UART write. The kernel's
    # explicit SysRq reset record is independent evidence of the same event;
    # the second boot must still change boot ID and restore the original bytes.
    if phase==1 and f'DMITBOX_{backend.upper()}_FORCED_REBOOT' not in text and 'sysrq: Resetting' not in text:
        raise RuntimeError(f'{backend} boot1: no forced-reset evidence')
    match=re.search(r'BOOT_FIXTURE_START pid1=\S+ boot=([a-f0-9-]{36})',text)
    if not match:raise RuntimeError('No kernel boot identity observed')
    return match[1]
def backend_test(backend):
    disk=report/f'{backend}-state.ext4'
    with disk.open('wb') as f:f.truncate(256*1024*1024)
    subprocess.run([str(MKFS),'-q','-F','-t','ext4',str(disk)],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    first=boot(backend,disk,1);second=boot(backend,disk,2)
    if first==second:raise RuntimeError(backend+': kernel boot identity did not change')
    disk.unlink() # test disks may contain snapshots and challenges; logs suffice.
    return {'backend':backend,'status':'PASS','boot_ids':[first,second],'assertions':['real console initiating identity','public CLI prepares and applies owned file probe','independent readiness before apply','forced kernel reboot with pending transaction','boot-time rollback and content readback','actual deadline rollback after forward/backward guest wall-clock changes','ENOSPC/read-only write rejection preserves bytes']}
results=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
    futures={pool.submit(backend_test,b):b for b in ('systemd','openrc')}
    for future,backend in list(futures.items()):
        try:results.append(future.result())
        except Exception as exc:results.append({'backend':backend,'status':'FAIL','reason':str(exc)})
manifest['results']=results
manifest['artifact_unchanged']=artifact_hash==digest(ARTIFACT)
(report/'results.json').write_text(json.dumps(manifest,indent=2)+'\n')
for row in results:print(row['status']+' minimal '+row['backend']+' fixture'+(': '+row['reason'] if 'reason' in row else ''))
print('Fixture evidence: '+str(report/'results.json'))
print('UNVERIFIED: full Debian/Arch/Alpine manager matrix, external reachability and KVM')
sys.exit(0 if all(r['status']=='PASS' for r in results) and manifest['artifact_unchanged'] else 1)
