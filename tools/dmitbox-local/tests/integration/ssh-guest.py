#!/usr/bin/env python3
"""Development-only actual SSH confirmation primitive test in a disposable guest.
Uses prepared local QEMU/kernel cache; no downloads or host privileged mutations.
This is not a full SSH/nftables port-migration or distribution certification.
"""
from pathlib import Path
import fcntl, gzip, hashlib, json, lzma, os, re, shutil, stat, subprocess, sys, time
ROOT=Path(__file__).resolve().parents[2]
CACHE=ROOT/'.cache/vm-validation'
CACHE.mkdir(parents=True,exist_ok=True)
# The cloned root, host-forward port, keys and control sockets are shared
# development fixtures. Serialize the entire run before any cache mutation.
fixture_lock=(CACHE/'native-ssh.lock').open('a')
fcntl.flock(fixture_lock,fcntl.LOCK_EX)
R=CACHE/'ssh-root'
OUT=ROOT/'artifacts/vm-validation'
TOOLS=CACHE/'tools'
OUT.mkdir(parents=True,exist_ok=True)
REQUIRED=[CACHE/'systemd-root/init',TOOLS/'usr/bin/qemu-system-x86_64',TOOLS/'boot/vmlinuz-6.12.107+deb13-amd64',Path('/usr/sbin/sshd'),Path('/usr/bin/ssh')]
for p in REQUIRED:
    if not p.exists():
        print('SKIP SSH guest prerequisite missing:',p);sys.exit(77)
if R.exists():shutil.rmtree(R)
shutil.copytree(CACHE/'systemd-root',R,symlinks=True)
# This cloned fixture tree is private and disposable. Source modules are copied
# once and hashed in evidence, not changed in the parent tree.
shutil.copytree(ROOT/'src',R/'opt/src',dirs_exist_ok=True)
seen=set()
def binary(p,dest=None):
    p=Path(p);dest=dest or str(p).lstrip('/');target=R/dest
    if str(p) in seen and target.exists():return
    seen.add(str(p));target.parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(p.resolve(),target)
    out=subprocess.run(['ldd',str(p)],capture_output=True,text=True).stdout
    for lib in re.findall(r'(?:=>\s*)?(/[A-Za-z0-9_./+-]+)',out):
        if Path(lib).is_file():binary(lib)
for p in ['/usr/sbin/sshd','/usr/lib/openssh/sshd-session','/usr/lib/openssh/sshd-auth']:
    binary(p)
(R/'root').mkdir(exist_ok=True);(R/'root/.ssh').mkdir(exist_ok=True);(R/'root/.ssh').chmod(0o700)
key=CACHE/'ssh-client-key'
if not key.exists():subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(key)],check=True)
shutil.copy2(Path(str(key)+'.pub'),R/'root/.ssh/authorized_keys');(R/'root/.ssh/authorized_keys').chmod(0o600)
hostkey=CACHE/'ssh-host-key'
if not hostkey.exists():subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(hostkey)],check=True)
(R/'etc/ssh').mkdir(exist_ok=True);shutil.copy2(hostkey,R/'etc/ssh/ssh_host_ed25519_key');(R/'etc/ssh/ssh_host_ed25519_key').chmod(0o600)
(R/'etc/passwd').write_text('root::0:0:root:/root:/bin/bash\nsshd:x:74:74:sshd:/run/sshd:/usr/sbin/nologin\n')
(R/'etc/group').write_text('root:x:0:\nsshd:x:74:\n')
(R/'etc/ssh/sshd_config').write_text('Port 2222\nListenAddress 0.0.0.0\nHostKey /etc/ssh/ssh_host_ed25519_key\nPermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nAuthorizedKeysFile /root/.ssh/authorized_keys\nPidFile /run/sshd.pid\nLogLevel VERBOSE\n')
# Native network driver modules from the cache's official Debian kernel package.
mods=TOOLS/'usr/lib/modules/6.12.107+deb13-amd64/kernel'
mp={p.name[:-6].replace('-','_'):p for p in mods.rglob('*.ko.xz')};loaded=[]
def module(name):
    if name in loaded or name not in mp:return
    data=lzma.decompress(mp[name].read_bytes());m=re.search(rb'(?:\x00|^)depends=([^\x00]*)',data)
    if m:
        for dep in m[1].decode().split(','):
            if dep:module(dep.replace('-','_'))
    (R/'opt/modules'/f'{name}.ko').write_bytes(data);loaded.append(name)
module('virtio_net')
with (R/'opt/module-load.sh').open('a') as f:
    for name in loaded:f.write('/bin/insmod /opt/modules/'+name+'.ko\n')
# No disk is required: all transaction/proof fixture files remain in guest RAM.
(R/'init').write_text('''#!/bin/busybox sh
export PATH=/usr/bin:/usr/sbin:/bin:/sbin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
ln -s /proc/self/fd /dev/fd
mkdir -p /dev/pts
mount -t devpts -o gid=5,mode=620 devpts /dev/pts
mount -t tmpfs -o mode=0755 tmpfs /run
mkdir -p /sys/fs/cgroup /run/sshd /var/lib/dmitbox-local/enrollments
chmod 700 /var/lib/dmitbox-local /var/lib/dmitbox-local/enrollments
mount -t cgroup2 cgroup2 /sys/fs/cgroup
/bin/sh /opt/module-load.sh
sleep 1
/usr/bin/ip link set lo up
iface=$(/usr/bin/ip -j link show | jq -r '.[]|select(.ifname!="lo")|.ifname')
/usr/bin/ip address add 10.0.2.15/24 dev "$iface"
/usr/bin/ip link set "$iface" up
exec /usr/lib/systemd/systemd --system --unit=multi-user.target --log-level=warning
''');(R/'init').chmod(0o755)
for path in ['etc/systemd/system/dmitbox-fixture.service','etc/systemd/system/multi-user.target.wants/dmitbox-fixture.service']:
    p=R/path
    if p.exists() or p.is_symlink():p.unlink()
(R/'etc/systemd/system/ssh-fixture.service').write_text('''[Unit]
After=local-fs.target
[Service]
Type=simple
ExecStart=/usr/sbin/sshd -D -e
[Install]
WantedBy=multi-user.target
''')
(R/'etc/systemd/system/multi-user.target.wants/ssh-fixture.service').symlink_to('../ssh-fixture.service')
manifest={'schema':1,'contexts':[{'user':'root','host':'fixture','addr':'10.0.2.2','laddr':'10.0.2.15','lport':2222}],'confirmation':{'sshd_binaries':[],'ssh_cgroups':['/system.slice/ssh-fixture.service'],'console_binaries':[],'console_cgroups':[],'console_devices':[]}}
for p in ['/usr/sbin/sshd','/usr/lib/openssh/sshd-session','/usr/lib/openssh/sshd-auth']:
    manifest['confirmation']['sshd_binaries'].append({'path':p,'sha256':hashlib.sha256((R/p.lstrip('/')).read_bytes()).hexdigest()})
(R/'var/lib/dmitbox-local/enrollments').mkdir(parents=True,exist_ok=True)
(R/'var/lib/dmitbox-local/enrollments/ssh.json').write_text(json.dumps(manifest))
COMMON='''#!/bin/bash
set -u -o pipefail
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH
DMB_STATE_DIR=/var/lib/dmitbox-local
DMB_RUNTIME_DIR=/run/dmitbox-local
source /opt/src/core.sh
source /opt/src/validation.sh
source /opt/src/confirmation.sh
'''
(R/'opt/ssh-origin.sh').write_text(COMMON+'dmb_confirmation_transport\n')
(R/'opt/ssh-check.sh').write_text(COMMON+'dmb_confirmation_check /var/lib/dmitbox-local/transactions/tx-00000000000000000000000000000000 ssh\n')
challenge='feedface'*8
(R/'opt/ssh-prepare.sh').write_text(COMMON+'''dir=/var/lib/dmitbox-local/transactions/tx-00000000000000000000000000000000
mkdir -p "$dir"
chmod 700 "$dir"
origin=$(cat /tmp/origin.json)
jq -cn --argjson origin "$origin" '{origin_transport:$origin,confirm_user:"root",confirm_port:2222,confirmation_enrollment:"ssh"}' > "$dir/plan.json"
now=$(dmb_confirmation_now_ticks)
up=$(cut -d' ' -f1 /proc/uptime);up=${up%%.*}
jq -cn --arg boot "$(cat /proc/sys/kernel/random/boot_id)" --argjson now "$now" --argjson deadline "$((up+90))" '{id:"tx-00000000000000000000000000000000",generation:1,confirmation_round:0,boot_id:$boot,confirmation_min_start_ticks:$now,deadline_uptime:$deadline}' > "$dir/state.json"
printf %s '''+challenge+''' | sha256sum | cut -d' ' -f1 > "$dir/challenge.sha256"
printf 'PREPARED_TRANSPORT_BOUNDARY=%s\\n' "$now"
''')
for p in (R/'opt').glob('ssh-*.sh'):p.chmod(0o700)
# Root ownership in the archive is encoded without mutating host ownership.
archive=bytearray();ino=1
def entry(name,mode,data=b'',major=0,minor=0):
    global ino
    nb=name.encode()+b'\0';fields=[ino,mode,0,0,1,0,len(data),0,0,major,minor,len(nb),0];ino+=1
    archive.extend(b'070701'+b''.join(f'{v:08x}'.encode() for v in fields));archive.extend(nb);archive.extend(b'\0'*((-len(archive))%4));archive.extend(data);archive.extend(b'\0'*((-len(archive))%4))
for p in sorted(R.rglob('*')):
    n=str(p.relative_to(R));s=p.lstat()
    if p.is_symlink():entry(n,stat.S_IFLNK|0o777,os.readlink(p).encode())
    elif p.is_dir():entry(n,stat.S_IFDIR|(s.st_mode&0o777))
    elif p.is_file():entry(n,stat.S_IFREG|(s.st_mode&0o777),p.read_bytes())
entry('dev/console',stat.S_IFCHR|0o600,major=5,minor=1);entry('dev/null',stat.S_IFCHR|0o666,major=1,minor=3);entry('TRAILER!!!',0)
initrd=CACHE/'ssh-initramfs.cpio.gz'
with gzip.GzipFile(filename=str(initrd),mode='wb',mtime=0) as f:f.write(archive)
port=22222
known=CACHE/'ssh-known-hosts'
known.write_text(f'[127.0.0.1]:{port} '+Path(str(hostkey)+'.pub').read_text())
base=['ssh','-i',str(key),'-p',str(port),'-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(known),'-o','GlobalKnownHostsFile=/dev/null','-o','IdentitiesOnly=yes','-o','IdentityAgent=none','-o','LogLevel=ERROR','-o','ConnectTimeout=3']
env=os.environ.copy();env['LD_LIBRARY_PATH']=str(TOOLS/'usr/lib/x86_64-linux-gnu')
qemu=[str(TOOLS/'usr/bin/qemu-system-x86_64'),'-L',str(TOOLS/'usr/share/qemu'),'-bios',str(TOOLS/'usr/share/seabios/bios-256k.bin'),'-vga','none','-accel','tcg,thread=single','-m','512','-smp','1','-nographic','-monitor','none','-no-reboot','-kernel',str(TOOLS/'boot/vmlinuz-6.12.107+deb13-amd64'),'-initrd',str(initrd),'-append','console=ttyS0 rdinit=/init panic=5 quiet','-netdev',f'user,id=dmbnet,restrict=on,hostfwd=tcp:127.0.0.1:{port}-:2222','-device','virtio-net-pci,netdev=dmbnet']
results=[]
def remote(cmd,old=None,tty=False,input=None):
    opts=['-S',str(old)] if old else ['-o','ControlMaster=no','-o','ControlPath=none']
    if tty:opts+=['-tt']
    r=subprocess.run(base+opts+['root@127.0.0.1',cmd],input=input,capture_output=True,text=True,timeout=30)
    return r
with (OUT/'ssh-guest-boot.log').open('w') as bootlog:
    vm=subprocess.Popen(qemu,stdin=subprocess.DEVNULL,stdout=bootlog,stderr=subprocess.STDOUT,env=env)
    masters=[]
    try:
        for attempt in range(60):
            if vm.poll() is not None:raise RuntimeError('QEMU exited before ssh readiness')
            if remote('true').returncode==0:break
            time.sleep(1)
        else:raise RuntimeError('SSH guest did not become ready')
        for label in ['origin','preexisting']:
            sock=CACHE/f'ssh-control-{label}'
            if sock.exists():sock.unlink()
            subprocess.run(base+['-M','-S',str(sock),'-f','-N','root@127.0.0.1'],check=True,timeout=20)
            masters.append(sock)
        r=remote('set -o pipefail; bash /opt/ssh-origin.sh | tee /tmp/origin.json',masters[0]);assert r.returncode==0,(r.returncode,r.stderr)
        origin=json.loads(r.stdout);assert origin['kind']=='ssh' and origin['local_port']==2222 and origin['login_user']=='root' and origin['login_uid']==0
        results.append({'case':'real-native-sshd-socket-proof','pass':True,'proof':origin})
        # Establish that the second old transport is provable before the
        # policy boundary; a later generic identity failure is not an age test.
        r=remote('bash /opt/ssh-origin.sh',masters[1]);assert r.returncode==0,(r.returncode,r.stdout,r.stderr)
        older_origin=json.loads(r.stdout)
        assert older_origin['kind']=='ssh' and older_origin['login_user']=='root' and older_origin['login_uid']==0
        assert older_origin['socket_inode']!=origin['socket_inode']
        results.append({'case':'older-different-transport-kernel-proof','pass':True,'proof':older_origin})
        r=remote('sleep 1; bash /opt/ssh-prepare.sh');assert r.returncode==0,(r.returncode,r.stdout,r.stderr)
        for label,sock in zip(['same-transport','older-different-transport'],masters):
            r=remote('SSH_CONNECTION="forged 1 2 3" bash /opt/ssh-check.sh',sock,tty=True,input=challenge+'\n')
            assert r.returncode!=0 and 'connection predates' in r.stdout,(label,r.returncode,r.stdout,r.stderr)
            results.append({'case':label+'-rejected','pass':True,'exit_code':r.returncode,'stderr':r.stderr,'output':re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',r.stdout).replace(challenge,'[test challenge redacted]')})
        r=remote('bash /opt/ssh-check.sh',tty=True,input=challenge+'\n')
        assert r.returncode==0,(r.returncode,r.stdout,r.stderr)
        results.append({'case':'fresh-output-readback','pass':True,'output':re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',r.stdout).replace(challenge,'[test challenge redacted]')})
        proof=None
        plain=re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]','',r.stdout)
        for pos,char in enumerate(plain):
            if char!='{':continue
            try:
                candidate,_=json.JSONDecoder().raw_decode(plain[pos:])
                if isinstance(candidate,dict) and candidate.get('remote_access_verified') is True:proof=candidate;break
            except json.JSONDecodeError:pass
        assert proof and proof['transport']['local_port']==2222 and proof['transport']['login_user']=='root' and proof['transport']['login_uid']==0,(r.stdout,r.stderr)
        results.append({'case':'fresh-authentication-challenge-confirmed','pass':True,'proof':proof})
        # A known daemon and socket do not authorize an unenrolled account.
        r=remote("jq '.contexts[0].user=\"sshd\"' /var/lib/dmitbox-local/enrollments/ssh.json > /tmp/wrong-context.json && mv /tmp/wrong-context.json /var/lib/dmitbox-local/enrollments/ssh.json")
        assert r.returncode==0,(r.returncode,r.stderr)
        r=remote('bash /opt/ssh-origin.sh')
        assert r.returncode!=0 and not r.stdout.strip(),(r.returncode,r.stdout,r.stderr)
        results.append({'case':'authenticated-account-missing-from-enrollment-rejected','pass':True,'exit_code':r.returncode})
        print('PASS actual OpenSSH 10 kernel socket/account proof, original/older transport refusal, fresh challenge confirmation, unenrolled-account refusal')
        print('UNVERIFIED full port migration, native nft coordination, internet reachability and distribution manager matrix')
    except Exception as e:
        results.append({'case':'guest-error','pass':False,'error':str(e)})
        raise
    finally:
        for sock in masters:
            subprocess.run(base+['-S',str(sock),'-O','exit','root@127.0.0.1'],capture_output=True,timeout=10)
        vm.terminate()
        try:vm.wait(10)
        except subprocess.TimeoutExpired:vm.kill();vm.wait()
        source_hashes={'src/'+str(p.relative_to(R/'opt/src')):hashlib.sha256(p.read_bytes()).hexdigest() for p in (R/'opt/src').rglob('*.sh')}
        (OUT/'ssh-guest-results.json').write_text(json.dumps({'schema':1,'classification':'minimal systemd native OpenSSH fixture using QEMU TCG; confirmation primitives only','source_sha256':source_hashes,'results':results},indent=2)+'\n')
