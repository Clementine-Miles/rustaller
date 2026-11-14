#!/usr/bin/env python3
"""Build disposable minimal init fixtures from explicitly prepared native inputs.

No downloads or host mounts are performed. This is a development tool, never a
runtime requirement. These are systemd/OpenRC fixtures, not full distro images.
"""
from pathlib import Path
import argparse, os, shutil, subprocess, re, lzma, gzip, stat

ROOT=Path(__file__).resolve().parents[2]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cache',type=Path,default=ROOT/'.cache/vm-validation')
parser.add_argument('--artifact',type=Path,default=ROOT/'dist/dmitbox-local.sh')
parser.add_argument('--busybox',type=Path,required=True,help='Reviewed static BusyBox binary')
parser.add_argument('--alpine-root',type=Path,help='Prepared Alpine root with native Bash/jq/coreutils/acl/attr/util-linux/OpenRC')
parser.add_argument('--kernel-version',default='6.12.107+deb13-amd64')
args=parser.parse_args()
P=args.cache.resolve()
ARTIFACT=args.artifact.resolve()
BUSYBOX=args.busybox.resolve()
KERNEL_VERSION=args.kernel_version
if not ARTIFACT.is_file() or not BUSYBOX.is_file():
    raise SystemExit('Artifact/static BusyBox input is missing')
P.mkdir(parents=True,exist_ok=True)
R=P/'systemd-root'

if R.exists():shutil.rmtree(R)
R.mkdir()
for d in ['usr/bin','usr/sbin','usr/lib/systemd/system','usr/lib/systemd','etc/systemd/system','etc','proc','sys','dev','run','tmp','var/lib','var/log','opt','persistent','usr/local']:(R/d).mkdir(parents=True,exist_ok=True)
for a,b in [('bin','usr/bin'),('sbin','usr/sbin'),('lib','usr/lib'),('lib64','usr/lib64')]:
 p=R/a
 if not p.exists():p.symlink_to(b)
(R/'usr/lib64').mkdir(exist_ok=True)
seen=set()
def binary(path,dest=None):
 path=Path(path); dest=dest or str(path).lstrip('/'); target=R/dest; target.parent.mkdir(parents=True,exist_ok=True)
 if str(path) in seen and target.exists():return
 seen.add(str(path));shutil.copy2(path.resolve(),target)
 out=subprocess.run(['ldd',str(path)],capture_output=True,text=True).stdout
 for lib in re.findall(r'(?:=>\s*)?(/[A-Za-z0-9_./+-]+)',out):
  if Path(lib).is_file():binary(lib)
for name in ['bash','jq','flock','timeout','setsid','sha256sum','sync','stat','mktemp','cp','mv','ln','rm','chmod','chown','getfacl','setfacl','base64','sed','sort','cut','od','tr','awk','readlink','grep','find','head','tail','cat','mkdir','sleep','systemctl','date','tee','kill','ip','ss','getconf']:
 p=shutil.which(name)
 if p:binary(p,'usr/bin/'+name)
for name in ['getfattr','setfattr']:binary(P/'tools/usr/bin'/name,'usr/bin/'+name)
binary('/usr/lib/systemd/systemd','usr/lib/systemd/systemd')
for name in ['systemd-shutdown','systemd-sysctl','systemd-modules-load','systemd-executor','systemd-journald','systemd-remount-fs','systemd-tmpfiles']:
 p=Path('/usr/lib/systemd')/name
 if p.exists():binary(p,'usr/lib/systemd/'+name)
for name in ['libnss_files.so.2','libnss_dns.so.2']:
 p=Path('/lib/x86_64-linux-gnu')/name
 if p.exists():binary(p)
shutil.copy2(BUSYBOX,R/'usr/bin/busybox')
for name in ['sh','mount','umount','poweroff','reboot','insmod','cttyhack','hostname','mknod']:
 p=R/'usr/bin'/name
 if not p.exists():p.symlink_to('busybox')
shutil.copy2(ARTIFACT,R/'opt/dmitbox-local.sh')
shutil.copytree(ROOT/'src',R/'opt/src',dirs_exist_ok=True)
shutil.copytree('/usr/lib/systemd/system',R/'usr/lib/systemd/system',dirs_exist_ok=True,symlinks=True)
for d in (R/'usr/lib/systemd/system').glob('*.target.wants'):shutil.rmtree(d)
for fname,data in {'etc/passwd':'root:x:0:0:root:/root:/bin/bash\n','etc/group':'root:x:0:\n','etc/os-release':Path('/etc/os-release').read_text(),'etc/machine-id':'04a409d0bed8460eaad99d08fa6d6eb9\n','etc/hostname':'dmitbox-systemd-fixture\n','etc/fstab':'','etc/nsswitch.conf':'passwd: files\ngroup: files\nhosts: files dns\n'}.items():(R/fname).write_text(data)
mods=P/'tools/usr/lib/modules'/KERNEL_VERSION/'kernel'; mp={x.name[:-6].replace('-','_'):x for x in mods.rglob('*.ko.xz')}; loaded=[]
def module(name):
 if name in loaded or name not in mp:return
 p=mp[name]; data=lzma.decompress(p.read_bytes()); m=re.search(rb'(?:\x00|^)depends=([^\x00]*)',data)
 if m:
  for dep in m[1].decode().split(','):
   if dep:module(dep.replace('-','_'))
 (R/'opt/modules').mkdir(exist_ok=True);(R/'opt/modules'/f'{name}.ko').write_bytes(data);loaded.append(name)
for m in ['virtio_pci','virtio_blk','crc32c_generic','ext4']:module(m)
(R/'opt/module-load.sh').write_text('\n'.join('/bin/insmod /opt/modules/'+m+'.ko' for m in loaded)+'\n')
(R/'init').write_text('''#!/bin/busybox sh
export PATH=/usr/bin:/usr/sbin:/bin:/sbin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mount -t tmpfs -o mode=0755 tmpfs /run
mkdir -p /sys/fs/cgroup
mount -t cgroup2 cgroup2 /sys/fs/cgroup
/bin/sh /opt/module-load.sh
sleep 2
/bin/busybox ls -l /dev/vd*
cat /proc/partitions
/bin/busybox dmesg | /bin/busybox tail -20
mount -t ext4 /dev/vda /persistent || { echo DMITBOX_VM_DISK_FAIL; poweroff -f; }
mkdir -p /persistent/var-lib /persistent/usr-local /persistent/etc-systemd
mount --bind /persistent/var-lib /var/lib
mount --bind /persistent/usr-local /usr/local
if [ ! -f /persistent/fixture-initialized ]; then
  cp -a /etc/systemd/system/. /persistent/etc-systemd/ || exit 1
  : > /persistent/fixture-initialized
fi
mount --bind /persistent/etc-systemd /etc/systemd/system
mkdir -p /run/lock
exec /usr/lib/systemd/systemd --system --unit=multi-user.target --log-level=warning
''');(R/'init').chmod(0o755)
(R/'etc/systemd/system/dmitbox-fixture.service').write_text('''[Unit]
Description=DMITBox native recovery test fixture
After=local-fs.target dmitbox-recovery-boot.service
[Service]
Type=oneshot
ExecStart=/bin/bash /opt/guest-test.sh
StandardInput=tty-force
StandardOutput=tty
StandardError=tty
TTYPath=/dev/ttyS0
TimeoutStartSec=180
[Install]
WantedBy=multi-user.target
''')
p=R/'etc/systemd/system/multi-user.target.wants';p.mkdir(exist_ok=True)
q=p/'dmitbox-fixture.service'
if not q.exists():q.symlink_to('../dmitbox-fixture.service')
(R/'opt/guest-test.sh').write_text('''#!/bin/bash
set -u -o pipefail
exec > >(tee /persistent/fixture.log) 2>&1
export PATH=/usr/bin:/usr/sbin:/bin:/sbin
printf 'BOOT_FIXTURE_START pid1=%s boot=%s\\n' "$(cat /proc/1/comm)" "$(cat /proc/sys/kernel/random/boot_id)"
phase=reboot
if [[ -f /persistent/reboot-pending ]]; then
  state=$(cat /var/lib/dmitbox-local/transactions/tx-*/state.json | jq -r .state)
  printf 'AFTER_REBOOT state=%s resource=%s\\n' "$state" "$(cat /var/lib/dmitbox-local/recovery-probe)"
  if [[ $state == ROLLED_BACK && $(cat /var/lib/dmitbox-local/recovery-probe) == before ]]; then printf 'DMITBOX_SYSTEMD_REBOOT_PASS\\n'; else printf 'DMITBOX_SYSTEMD_REBOOT_FAIL\\n'; sync; poweroff -f; fi
  phase=deadline
fi
mkdir -p /var/lib/dmitbox-local/enrollments
chmod 700 /var/lib/dmitbox-local /var/lib/dmitbox-local/enrollments
console_binary=$(readlink /proc/$$/exe)
console_hash=$(sha256sum "$console_binary" | cut -d' ' -f1)
console_group=$(awk -F: '$1==0 {print $3}' /proc/$$/cgroup)
console_device=$(stat -c '%t:%T' /dev/ttyS0)
jq -cn --arg path "$console_binary" --arg hash "$console_hash" --arg group "$console_group" --arg device "$console_device" '{schema:1,confirmation:{allow_console_confirmation:true,sshd_binaries:[],ssh_cgroups:[],console_binaries:[{path:$path,sha256:$hash}],console_cgroups:[$group],console_devices:[{path:"/dev/ttyS0",device:$device}]}}' > /var/lib/dmitbox-local/enrollments/ssh.json
chmod 600 /var/lib/dmitbox-local/enrollments/ssh.json
mkdir -p /mnt/full /mnt/readonly
mount -t tmpfs -o size=1m,mode=0755 tmpfs /mnt/full
printf original > /mnt/full/target
/bin/busybox dd if=/dev/zero of=/mnt/full/filler bs=1024 count=2048 2>/dev/null
( source /opt/src/core.sh; printf replacement | dmb_atomic_write /mnt/full/target 600 )
full_rc=$?
if ((full_rc!=0)) && [[ $(cat /mnt/full/target) == original ]]; then printf 'DMITBOX_VM_ENOSPC_PASS\\n'; else printf 'DMITBOX_VM_ENOSPC_FAIL\\n'; fi
umount /mnt/full
mount -t tmpfs -o size=1m,mode=0755 tmpfs /mnt/readonly
printf original > /mnt/readonly/target
mount -o remount,ro /mnt/readonly
( source /opt/src/core.sh; printf replacement | dmb_atomic_write /mnt/readonly/target 600 )
readonly_rc=$?
if ((readonly_rc!=0)) && [[ $(cat /mnt/readonly/target) == original ]]; then printf 'DMITBOX_VM_READONLY_PASS\\n'; else printf 'DMITBOX_VM_READONLY_FAIL\\n'; fi
umount /mnt/readonly
printf before > /var/lib/dmitbox-local/recovery-probe
chmod 600 /var/lib/dmitbox-local/recovery-probe
beforehash=$(sha256sum /var/lib/dmitbox-local/recovery-probe | cut -d' ' -f1)
afterhash=$(printf after | sha256sum | cut -d' ' -f1)
acl=$(getfacl -cpn /var/lib/dmitbox-local/recovery-probe | base64 -w0)
jq -cn --arg beforehash "$beforehash" --arg afterhash "$afterhash" --arg acl "$acl" '{schema:1,adapter:"fs",resources:["file:persistent-resource"],timeout_seconds:180,origin_transport:null,allow_console_confirmation:true,steps:[{id:"file-change",purpose:"recovery-probe",resource:"file:persistent-resource",adapter:"fs",path:"/var/lib/dmitbox-local/recovery-probe",content_b64:"YWZ0ZXI=",before:{exists:true,type:"file",sha256:$beforehash,mode:"600",uid:0,gid:0,acl_b64:$acl,xattrs_b64:""},after:{exists:true,type:"file",sha256:$afterhash,mode:"600",uid:0,gid:0,acl_b64:$acl,xattrs_b64:""}}]}' > /persistent/plan.json
if [[ $phase == deadline ]]; then jq '.timeout_seconds=60' /persistent/plan.json > /persistent/deadline-plan.json; cp /persistent/deadline-plan.json /persistent/plan.json; fi
/bin/bash /opt/dmitbox-local.sh --apply-plan /persistent/plan.json > /persistent/apply.trace 2>&1
rc=$?
if ((rc!=0)); then tail -180 /persistent/apply.trace; fi
printf 'APPLY_RC=%s resource=%s\\n' "$rc" "$(cat /var/lib/dmitbox-local/recovery-probe)"
if ((rc!=0)) || [[ $(cat /var/lib/dmitbox-local/recovery-probe) != after ]]; then printf 'DMITBOX_SYSTEMD_APPLY_FAIL\\n'; sync; poweroff -f; fi
if [[ $phase == deadline ]]; then
  active=''
  for record in /var/lib/dmitbox-local/transactions/tx-*/state.json; do
    if [[ $(jq -r .state "$record") == VERIFIED ]]; then active=$record; fi
  done
  [[ -n $active ]] || { printf 'DMITBOX_SYSTEMD_DEADLINE_FAIL missing-active-record\\n'; sync; poweroff -f; }
  # Only this isolated guest's wall clock changes. Recovery must use uptime.
  # Linux rejects realtime values earlier than current CLOCK_MONOTONIC uptime;
  # year 2000 is a backwards jump that remains above that kernel lower bound.
  date -u -s '@1893456000' >/dev/null && date -u -s '@946684800' >/dev/null || { printf 'DMITBOX_VM_CLOCK_CHANGE_FAIL\\n'; sync; poweroff -f; }
  printf 'DMITBOX_VM_CLOCK_CHANGE_PASS\\n'
  read -r start rest < /proc/uptime; start=${start%%.*}
  while :; do
    state=$(jq -r .state "$active")
    [[ $state != ROLLED_BACK ]] || break
    read -r now rest < /proc/uptime; now=${now%%.*}
    ((now-start<85)) || break
    sleep 1
  done
  if [[ $state == ROLLED_BACK && $(cat /var/lib/dmitbox-local/recovery-probe) == before ]]; then printf 'DMITBOX_SYSTEMD_DEADLINE_PASS\\n'; else printf 'DMITBOX_SYSTEMD_DEADLINE_FAIL state=%s\\n' "$state"; fi
  sync; poweroff -f
fi
printf pending > /persistent/reboot-pending
sync
printf 'DMITBOX_SYSTEMD_FORCED_REBOOT\\n'
echo b > /proc/sysrq-trigger
''')
# Build newc archive with root IDs without requiring host chown/mknod.
archive=bytearray();ino=1
def entry(name,mode,data=b'',major=0,minor=0):
 global ino
 nb=name.encode()+b'\0'; fields=[ino,mode,0,0,1,0,len(data),0,0,major,minor,len(nb),0];ino+=1
 archive.extend(b'070701'+b''.join(f'{v:08x}'.encode() for v in fields));archive.extend(nb);archive.extend(b'\0'*((-len(archive))%4));archive.extend(data);archive.extend(b'\0'*((-len(archive))%4))
for p in sorted(R.rglob('*')):
 n=str(p.relative_to(R));s=p.lstat()
 if p.is_symlink():entry(n,stat.S_IFLNK|0o777,os.readlink(p).encode())
 elif p.is_dir():entry(n,stat.S_IFDIR|0o755)
 elif p.is_file():entry(n,stat.S_IFREG|(s.st_mode&0o777),p.read_bytes())
entry('dev/console',stat.S_IFCHR|0o600,major=5,minor=1);entry('dev/null',stat.S_IFCHR|0o666,major=1,minor=3);entry('TRAILER!!!',0)
with gzip.GzipFile(filename=str(P/'systemd-initramfs.cpio.gz'),mode='wb',mtime=0) as f:f.write(archive)
print('Built',len(archive),'bytes uncompressed, modules',loaded)


if args.alpine_root:
    source_root=args.alpine_root.resolve()
    R=P/'openrc-root'
    if source_root==R:
        raise SystemExit('Use a separate prepared Alpine input root')
    if R.exists(): shutil.rmtree(R)
    shutil.copytree(source_root,R,symlinks=True)
    
    for d in ['proc','sys','dev','run','tmp','var/lib','opt','persistent','usr/local','etc/runlevels/sysinit','etc/runlevels/boot','etc/runlevels/default']:(R/d).mkdir(parents=True,exist_ok=True)
    for d in ['sysinit','boot','default']:
     for p in (R/'etc/runlevels'/d).iterdir():
      if p.is_symlink() or p.is_file():p.unlink()
    shutil.copytree(P/'systemd-root/opt/modules',R/'opt/modules',dirs_exist_ok=True)
    shutil.copy2(P/'systemd-root/opt/module-load.sh',R/'opt/module-load.sh')
    if not (R/'bin/insmod').exists():(R/'bin/insmod').symlink_to('busybox')
    shutil.copytree(ROOT/'src',R/'opt/src',dirs_exist_ok=True)
    shutil.copy2(ARTIFACT,R/'opt/dmitbox-local.sh')
    shutil.copy2(BUSYBOX,R/'opt/busybox')
    (R/'init').write_text('''#!/bin/sh
    export PATH=/usr/bin:/usr/sbin:/bin:/sbin
    mount -t proc proc /proc
    mount -t sysfs sysfs /sys
    mount -t devtmpfs devtmpfs /dev
    ln -s /proc/self/fd /dev/fd
    mkdir -p /sys/fs/cgroup
    mount -t cgroup2 cgroup2 /sys/fs/cgroup
    mount -t tmpfs -o mode=0755 tmpfs /run
    /bin/sh /opt/module-load.sh
    sleep 2
    mount -t ext4 /dev/vda /persistent || { echo DMITBOX_VM_DISK_FAIL; poweroff -f; }
    mkdir -p /persistent/var-lib /persistent/usr-local /persistent/etc-init /persistent/etc-levels
    mount --bind /persistent/var-lib /var/lib
    mount --bind /persistent/usr-local /usr/local
    if [ ! -f /persistent/fixture-initialized ]; then
      cp -a /etc/init.d/. /persistent/etc-init/ || exit 1
      cp -a /etc/runlevels/. /persistent/etc-levels/ || exit 1
      : > /persistent/fixture-initialized
    fi
    mount --bind /persistent/etc-init /etc/init.d
    mount --bind /persistent/etc-levels /etc/runlevels
    mkdir -p /run/lock
    exec /sbin/init
    ''');(R/'init').chmod(0o755)
    (R/'etc/inittab').write_text('''::sysinit:/sbin/openrc sysinit
    ::sysinit:/sbin/openrc boot
    ::wait:/sbin/openrc default
    ::shutdown:/sbin/openrc shutdown
    ''')
    (R/'etc/init.d/localmount').write_text('''#!/sbin/openrc-run
    description="Fixture filesystems mounted by initramfs"
    start() { return 0; }
    ''');(R/'etc/init.d/localmount').chmod(0o755)
    (R/'etc/runlevels/boot/localmount').symlink_to('/etc/init.d/localmount')
    (R/'etc/init.d/dmitbox-fixture').write_text('''#!/sbin/openrc-run
    description="Native DMITBox OpenRC recovery fixture"
    depend() { need localmount; after dmitbox-recovery-boot; }
    start() { /opt/busybox setsid /opt/busybox cttyhack /bin/bash /opt/guest-test.sh </dev/ttyS0 >/dev/ttyS0 2>&1; }
    ''');(R/'etc/init.d/dmitbox-fixture').chmod(0o755)
    (R/'etc/runlevels/default/dmitbox-fixture').symlink_to('/etc/init.d/dmitbox-fixture')
    s=(P/'systemd-root/opt/guest-test.sh').read_text().replace('SYSTEMD','OPENRC').replace('pid1=','pid1=')
    (R/'opt/guest-test.sh').write_text(s)
    if not (R/'dev/fd').exists(): (R/'dev/fd').symlink_to('/proc/self/fd')
    inittab=R/'etc/inittab'
    inittab.write_text('\n'.join(line.strip() for line in inittab.read_text().splitlines() if line.strip())+'\n')
    # remove foreign hostresolver/hosts from fixture (no guest networking requested)
    (R/'etc/resolv.conf').write_text('');(R/'etc/hosts').write_text('127.0.0.1 localhost\n::1 localhost\n')
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
    with gzip.GzipFile(filename=str(P/'openrc-initramfs.cpio.gz'),mode='wb',mtime=0) as f:f.write(archive)
    print('Built OpenRC initramfs',len(archive),'bytes')
else:
    print("OpenRC fixture not requested: --alpine-root was omitted")
