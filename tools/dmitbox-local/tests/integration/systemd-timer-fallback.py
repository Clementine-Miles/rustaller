#!/usr/bin/env python3
"""Actual monotonic timer recovery with its polling watcher deliberately paused.

Uses the explicitly prepared minimal fixture inputs from guest-matrix.sh. No NIC,
no host mount, and no host clock change. This is not a full distro acceptance.
"""
from pathlib import Path
import datetime,gzip,hashlib,json,os,re,shutil,stat,subprocess,sys,tempfile
ROOT=Path(__file__).resolve().parents[2]
CACHE=Path(os.environ.get('DMITBOX_DEV_VM_CACHE',ROOT/'.cache/vm-validation')).resolve()
TOOLS=CACHE/'tools';SOURCE=CACHE/'systemd-root';ARTIFACT=ROOT/'dist/dmitbox-local.sh'
if not (SOURCE/'opt/guest-test.sh').is_file():
    print('SKIP timer fallback: run prepared guest-matrix.sh first');sys.exit(77)
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
artifact_hash=digest(ARTIFACT)
if digest(SOURCE/'opt/dmitbox-local.sh')!=artifact_hash:
    print('SKIP timer fallback: prepared fixture artifact differs; rebuild guests');sys.exit(77)
report=Path(tempfile.mkdtemp(prefix='systemd-timer-alone-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-',dir=ROOT/'artifacts'))
R=report/'root';shutil.copytree(SOURCE,R,symlinks=True)
driver=R/'opt/guest-test.sh'
text=driver.read_text().replace('phase=reboot\n','phase=deadline\n',1)
needle='  # Only this isolated guest\'s wall clock changes. Recovery must use uptime.'
pause='''  id=${active%/state.json}; id=${id##*/}
  mainpid=$(systemctl show --property=MainPID --value "dmitbox-$id.service")
  [[ $mainpid =~ ^[1-9][0-9]*$ ]] || { printf 'DMITBOX_TIMER_WATCHER_PAUSE_FAIL\\n'; sync; poweroff -f; }
  kill -STOP "$mainpid" || { printf 'DMITBOX_TIMER_WATCHER_PAUSE_FAIL\\n'; sync; poweroff -f; }
  printf 'DMITBOX_TIMER_WATCHER_PAUSED_PASS\\n'
'''
if needle not in text:raise SystemExit('Fixture driver interface changed')
driver.write_text(text.replace(needle,pause+needle,1))
archive=bytearray();ino=1
def entry(name,mode,data=b'',major=0,minor=0):
    global ino
    nb=name.encode()+b'\0';fields=[ino,mode,0,0,1,0,len(data),0,0,major,minor,len(nb),0];ino+=1
    archive.extend(b'070701'+b''.join(f'{v:08x}'.encode() for v in fields));archive.extend(nb);archive.extend(b'\0'*((-len(archive))%4));archive.extend(data);archive.extend(b'\0'*((-len(archive))%4))
for p in sorted(R.rglob('*')):
    n=str(p.relative_to(R));s=p.lstat()
    if p.is_symlink():entry(n,stat.S_IFLNK|0o777,os.readlink(p).encode())
    elif p.is_dir():entry(n,stat.S_IFDIR|0o755)
    elif p.is_file():entry(n,stat.S_IFREG|(s.st_mode&0o777),p.read_bytes())
entry('dev/console',stat.S_IFCHR|0o600,major=5,minor=1);entry('dev/null',stat.S_IFCHR|0o666,major=1,minor=3);entry('TRAILER!!!',0)
initrd=report/'initramfs.cpio.gz'
with gzip.GzipFile(filename=str(initrd),mode='wb',mtime=0) as f:f.write(archive)
env=os.environ.copy();env['LD_LIBRARY_PATH']=str(TOOLS/'usr/lib/x86_64-linux-gnu')
disk=report/'state.ext4'
with disk.open('wb') as f:f.truncate(256*1024*1024)
subprocess.run([str(TOOLS/'usr/sbin/mke2fs'),'-q','-F','-t','ext4',str(disk)],env=env,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
args=[str(TOOLS/'usr/bin/qemu-system-x86_64'),'-L',str(TOOLS/'usr/share/qemu'),'-bios',str(TOOLS/'usr/share/seabios/bios-256k.bin'),'-vga','none','-accel','tcg,thread=single','-m','512','-smp','1','-nographic','-monitor','none','-no-reboot','-nic','none','-kernel',str(TOOLS/'boot/vmlinuz-6.12.107+deb13-amd64'),'-initrd',str(initrd),'-append','console=ttyS0 rdinit=/init panic=5 quiet','-drive',f'file={disk},format=raw,if=virtio']
log=report/'boot.log';rc=0
with log.open('w') as f:
    try:rc=subprocess.run(args,stdout=f,stderr=subprocess.STDOUT,stdin=subprocess.DEVNULL,env=env,timeout=180).returncode
    except subprocess.TimeoutExpired:rc=124
text=log.read_text(errors='replace')
log.write_text(re.sub(r'(One-use confirmation challenge \(keep private\):) [a-f0-9]{64}',r'\1 [REDACTED]',text))
markers=['APPLY_RC=0 resource=after','DMITBOX_TIMER_WATCHER_PAUSED_PASS','DMITBOX_VM_CLOCK_CHANGE_PASS','DMITBOX_SYSTEMD_DEADLINE_PASS']
success=rc==0 and all(marker in text for marker in markers) and digest(ARTIFACT)==artifact_hash
result={'schema':1,'classification':'minimal systemd timer-alone fixture under QEMU TCG; no full distro or network-access claim','artifact_sha256':artifact_hash,'artifact_unchanged':digest(ARTIFACT)==artifact_hash,'status':'PASS' if success else 'FAIL','qemu_exit_code':rc,'markers':{m:m in text for m in markers},'assertions':['armed public CLI file transaction reached intended after bytes','polling watcher MainPID deliberately SIGSTOPped','guest wall clock moved forwards and backwards','independent monotonic timer restored original file bytes and ROLLED_BACK state']}
(report/'results.json').write_text(json.dumps(result,indent=2)+'\n')
if success:
    shutil.rmtree(R);disk.unlink();initrd.unlink()
print(result['status']+' independent systemd timer fallback; evidence '+str(report/'results.json'))
sys.exit(0 if success else 1)
