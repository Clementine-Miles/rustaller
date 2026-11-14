#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
# shellcheck source=../src/validation.sh
source "$ROOT/src/validation.sh"
[[ $(dmb_ip_parse 192.0.2.1) == 192.0.2.1 ]]
[[ $(dmb_ip_parse 2001:0DB8:0:0:0:0:0:1 6) == 2001:db8::1 ]]
[[ $(dmb_ip_parse ::ffff:192.0.2.1) == ::ffff:c000:201 ]]
[[ $(dmb_cidr_parse 192.0.2.255/24 4) == 192.0.2.0/24 ]]
[[ $(dmb_cidr_parse 2001:db8:ffff::1/33) == 2001:db8:8000::/33 ]]
for invalid in 1 127.1 0177.0.0.1 256.0.0.1 1.2.3.4.5 ' 1.2.3.4' 1.2.3.04 'fe80::1%eth0' ':::' '1::2::3' '1:2:3:4:5:6:7:8:9' '::ffff:192.0.2.999'; do
  if dmb_ip_parse "$invalid" >/dev/null; then printf 'Accepted invalid address %s\n' "$invalid" >&2; exit 1; fi
done
! dmb_ip_parse ::1 4 >/dev/null
! dmb_ip_parse 192.0.2.1 6 >/dev/null
! dmb_cidr_parse 1.2.3.4/33 >/dev/null
! dmb_cidr_parse ::/129 >/dev/null
dmb_ip_in_cidr 2001:db8::dead 2001:db8::/64
! dmb_ip_in_cidr 2001:db9::1 2001:db8::/64
# Independent standard-library oracle is development-only, not a runtime helper.
python3 -I - "$ROOT" <<'PY'
import ipaddress,random,subprocess,sys
r=random.Random(61429)
checks=[]
for width in (32,128):
    for _ in range(160):
        cls=ipaddress.IPv4Address if width==32 else ipaddress.IPv6Address
        addr=cls(r.getrandbits(width))
        cidr=f'{addr}/{r.randrange(width+1)}'
        checks.append((f'dmb_ip_parse {addr.exploded}',str(addr)))
        checks.append((f'dmb_cidr_parse {cidr}',str(ipaddress.ip_network(cidr,strict=False))))
script='source "$1/src/validation.sh"\n'+'\n'.join(c for c,_ in checks)
out=subprocess.check_output(['bash','-euc',script,'test',sys.argv[1]],text=True).splitlines()
assert len(out)==len(checks),(len(out),len(checks))
for got,(_,expected) in zip(out,checks):
    assert got==expected,(got,expected)
print(f'PASS {len(checks)} independent IP/CIDR oracle comparisons')
PY
