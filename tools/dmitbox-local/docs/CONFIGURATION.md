# Configuration and transaction workflow

Use the installed, reviewed artifact in a root-owned path. The examples below use `/usr/local/sbin/dmitbox-local`. Replace example users, addresses, interfaces, unit names and owner IDs with observed local values. Addresses from documentation ranges are placeholders; pool enrollment deliberately rejects such ranges.

Runtime dependencies are native Bash, jq, GNU coreutils, util-linux and iproute2. Individual operations additionally require OpenSSH, nftables, ACL/xattr tools, the native DNS manager, or explicitly requested probe tools. Install dependencies through the distribution separately. `--check` reports availability without installing or loading anything. Supported recovery backends require a running systemd or OpenRC with `supervise-daemon`; an installed `systemctl` executable alone is insufficient. The [test evidence](TEST-EVIDENCE.md) identifies what was actually exercised.

Production paths are fixed: `/var/lib/dmitbox-local`, `/run/dmitbox-local` and root-owned native configuration paths. Environment variables cannot redirect privileged state or enable legacy unguarded mutation. Private parser simulations may use disposable files under `/run`; plan/status commands do not create durable transaction records or run connectivity probes.

## Review, apply, confirm and recover

```sh
/usr/local/sbin/dmitbox-local --net-report
/usr/local/sbin/dmitbox-local --plan mtu eth0 1400 runtime > mtu-plan.json
/usr/local/sbin/dmitbox-local --diff mtu-plan.json
/usr/local/sbin/dmitbox-local --apply-plan mtu-plan.json
```

Apply requires an enrolled, kernel-identified initiating SSH session or explicitly authorized console. The supplied plan's `origin_transport` cannot authorize confirmation: application replaces it with the actual origin. Plans are data and are never sourced as shell commands. Generic filesystem plans are restricted to supported purposes and exact owned paths; they cannot substitute for the SSH, firewall or DNS adapters.

Exit 0 means the requested command completed its documented postconditions. General failure or inconclusive evidence returns 1; invalid input uses 2/64, missing dependencies/unsupported capability 69, contention 75, conflict 76, privilege 77 and unsafe ownership/filesystem 78. Inspect the transaction record and proof scope; an exit status alone never certifies network accessibility.

Keep the displayed one-use challenge private. Apply prints a transaction ID and leaves a deadline running. From a newly authenticated SSH TCP connection made after application, run:

```sh
/usr/local/sbin/dmitbox-local --net-status tx-REPLACE_WITH_ID
/usr/local/sbin/dmitbox-local --net-confirm tx-REPLACE_WITH_ID ssh
```

SSH multiplexing through an older ControlMaster does not establish a new transport. Port migration first retains old and new listeners and allowances. The first proof retires the old path, issues another challenge and requires another fresh authentication. Both rounds share the original deadline. Local socket checks and valid configuration syntax never cancel recovery by themselves.

Confirmation must use the intended local account and the tested peer address, local address and listener port. `confirm_user` may select an enrolled account explicitly; otherwise an SSH-origin plan uses its actual login user. Console-origin plans use the sole enrolled user when unambiguous, or require an explicit user for SSH confirmation. An enrolled console can independently confirm console-authorized plans without asserting external accessibility. Protected application from namespaces differing from init is refused.

Explicit recovery uses `--net-rollback TXID`; it cancels the recorded mutator before taking the inverse path. A `CONFLICT` preserves a resource changed by another administrator. `ROLLBACK_FAILED` preserves failure evidence and requires an independently working access path. `ROLLED_BACK` records configuration/runtime postconditions and does not by itself prove external access. Committed historical changes are restored through a new reviewed inverse plan where the adapter supports that workflow.

## SSH enrollment

SSH service enrollment accepts a dedicated, already configured Port-only include. Establish its explicit anchor through your existing configuration process and verify access before enrollment. DMITBox does not rewrite the main configuration or adopt an arbitrary old fragment. For owner ID `11111111111111111111111111111111`, a fragment can contain:

```text
# dmitbox-local:11111111111111111111111111111111
Port 22
```

The main configuration must have exactly one explicit absolute `Include` for that fragment, outside unintended `Match` scope. Another glob must not also match it. Unrelated Include/Match files remain owned by their existing administrator. Active socket/inetd activation, ambiguous includes, independently owned effective Port directives and active Fail2Ban are currently refused.

Save an actual service description as `ssh-enrollment.json`:

```json
{
  "schema": 1,
  "owner_id": "11111111111111111111111111111111",
  "driver": "systemd-service",
  "service": "ssh.service",
  "sshd": "/usr/sbin/sshd",
  "config": "/etc/ssh/sshd_config",
  "fragment": "/etc/ssh/sshd_config.d/00-dmitbox-ports.conf",
  "contexts": [
    {"user":"admin","host":"198.51.100.20","addr":"198.51.100.20","laddr":"192.0.2.10","lport":22}
  ],
  "confirmation": {"allow_console_confirmation":false}
}
```

```sh
/usr/local/sbin/dmitbox-local --enroll ssh ssh-enrollment.json
```

Use `openrc-service`, the actual OpenRC service name and a trusted `/run/...` `pidfile` for OpenRC. Enrollment pins the installed daemon/session binaries, native service identity and configuration inventory. Each context must describe an intended user and connection; changing the port must preserve its effective authentication policy, including Match rules. A daemon package/configuration change can invalidate enrollment and requires review.

Port migration requires `UseDNS no` and `host` equal to the numeric peer `addr`. The kernel connection proof cannot certify a DNS-derived client hostname, so hostname-dependent contexts are refused rather than simulated under an unproven name.

An out-of-band workflow additionally needs `confirmation.allow_console_confirmation:true`, `console_devices:[{"path":"/dev/ttyS0","device":"4:40"}]`, `console_binaries:[{"path":"/usr/bin/login","sha256":"ACTUAL_SHA256"}]` and `console_cgroups:["ACTUAL_NATIVE_CGROUP"]` inside `confirmation`. These values must identify the real console's root-owned character device, trusted native login/getty executable and observed cgroup. The example device number is hexadecimal and must be replaced with `stat -c '%t:%T' /dev/ttyS0` output. A PTY, `/dev/tty` alias, SSH ancestry or a claimed environment variable does not prove console identity. Confirmation then uses `--net-confirm TXID console` and explicitly records that external reachability was not proved.

## Native firewall enrollment

Enrollment records explicit authority; it does not install policy. For an exclusive DMITBox INPUT table, save `nft-policy.json`:

```json
{
  "schema":1,
  "nft":{"mode":"exclusive","family":"inet","table":"dmitbox_local","chain":"input","parent_chain":null,"owner_id":"22222222222222222222222222222222"},
  "listeners":[{"address":"192.0.2.10","port":22}],
  "rules":[{"action":"allow","protocol":"tcp","port":443}],
  "policy":"drop"
}
```

```sh
/usr/local/sbin/dmitbox-local --enroll nft nft-policy.json
/usr/local/sbin/dmitbox-local --plan nft nft-policy.json > firewall-plan.json
/usr/local/sbin/dmitbox-local --diff firewall-plan.json
/usr/local/sbin/dmitbox-local --apply-plan firewall-plan.json
```

Include every intended listener, including applicable IPv6 and additional listening addresses. This is runtime policy; no persistent nftables loader is silently edited. Existing independently owned INPUT hooks or conflicting managers prevent exclusive ownership. The scoped alternative selects `mode:"scoped"`, the existing `inet` table, a `dmb_...` child chain and its existing INPUT `parent_chain`. The jump remains inside the same table; nftables cannot jump across tables. Enrollment and before-state guards must accept the existing pipeline. Unrelated tables, NAT, sets and container rules are not globally flushed.

After an owned firewall policy is established, port migration uses `--plan ssh-port 2222`, followed by the normal diff/apply/two-proof workflow. A firewall policy that merely admits the target port does not prove that external routing reaches it.

## DNS and link settings

```sh
/usr/local/sbin/dmitbox-local --enroll dns eth0
/usr/local/sbin/dmitbox-local --plan dns eth0 cloudflare persistent > dns-plan.json
/usr/local/sbin/dmitbox-local --plan dns eth0 custom persistent 9.9.9.9 2620:fe::fe
```

`cloudflare`, `google`, `quad9` and `custom` are supported. Each custom address is a separate numeric argument; empty input, duplicates, invalid families and unsupported scoped addresses are rejected. Persistent is the default; request `runtime` explicitly for a temporary native-manager override. Static `resolv.conf` needs explicit local enrollment; symlink targets are not rewritten. NetworkManager uses the actual connection UUID. Eligible networkd uses an existing owned DNS drop-in plus a resolved per-link override. Search/routing domains and DNSSEC/DoT policy are preserved; strict DoT changes needing server-name authority are refused. Unsupported generator/DHCP owners fail before mutation. DNS readback is distinct from an explicitly requested `--dns-probe NAME`.

MTU planning takes an explicit interface and optional routing/encapsulation context. For example, `--plan mtu tun0 1400 runtime '{"family":6,"encapsulation_overhead":80,"underlay_mtu":1500}'` declares the verified underlay constraint; it does not measure PMTU. `--pmtu-probe ADDRESS 4|6` is an explicit network operation. Interface up/down remains refused without a tested configuration-owner recovery adapter.

Static IPv6 enrollment uses `--enroll ipv6 eth0`, followed by `--plan ipv6 disable '{"iface":"eth0"}'`. Only supported unmanaged, forever-static address/route layouts qualify. Disable is split into individually journaled address/route/sysctl steps. An enable inverse may select the unique matching committed disable, or an explicit `transaction_id` in the context. Dynamic RA/DHCP, unknown managers and complex unverified layouts are refused rather than restored through sysctl values alone.

## Routed IPv6 pool authority

Pool activation needs a genuinely delegated global `/64`, an existing preferred permanent anchor, an unmanaged eligible link and an administrator-controlled external verifier. No verifier server or third-party script is installed. Save `pool-enrollment.json`, replacing every placeholder:

```json
{
  "schema":1,
  "id":"primary",
  "owner_id":"33333333333333333333333333333333",
  "prefix_authorized":true,
  "iface":"eth0",
  "prefix":"YOUR_DELEGATED_GLOBAL_PREFIX/64",
  "anchor":"YOUR_EXISTING_ANCHOR_ADDRESS",
  "verifier":{
    "path":"/usr/local/libexec/your-reviewed-pool-verifier",
    "sha256":"ACTUAL_SHA256",
    "identity":"separate-host-1",
    "args":[],
    "source_address":"EXTERNAL_VERIFIER_GLOBAL_IPV6_ADDRESS"
  }
}
```

```sh
/usr/local/sbin/dmitbox-local --enroll pool pool-enrollment.json
printf '%s\n' '{"enrollment":"primary","count":4,"external_probes":true}' > pool-plan-request.json
/usr/local/sbin/dmitbox-local --plan pool pool-plan-request.json > pool-plan.json
```

Planning fixes generated `/128` members without network probes. Application checks prefix/link/route identity, probes the existing anchor, performs DAD and requests external evidence for each member. The pinned verifier receives a JSON request on stdin and returns a JSON response bound to its request hash, nonce, phase, prefix, verifier identity and addresses. Its separate-host placement and honest observations are explicit trust assumptions, not facts a local shell can establish. A blocked or incomplete verifier response does not establish routability. Pools are runtime-only; renumbering and interface disappearance stop ownership checks. See [limitations](LIMITATIONS.md) for NAT66 support and validation boundaries.

## TCP, Swap and maintenance

Measured TCP plans use `--plan tcp bbr measurement-request.json runtime`; the request is `{"schema":1,"requested":true,"target":"NUMERIC_TEST_SERVER","port":5201,"seconds":5,"min_bits_per_second":0}`. The chosen native algorithm must already be available. Explicit iperf3 baseline/after measurements do not prove general workload improvement. Persistent TCP may create only its dedicated owned profile when no competing key owner exists; unrelated buffers, queues and sysctls are preserved.

`--plan swap /swapfile 1024 10` requests a new 1 GiB file at priority 10. Only verified supported local ext4/XFS layouts, a new path and same-filesystem staging qualify. Existing files are never overwritten; current support does not resize foreign Swap or edit fstab. Swapoff failure is reported and retains ownership evidence.

`--plan gai v4|default|restore` reviews glibc address selection through its dedicated policy file adapter. Existing independently owned precedence directives are refused. `--maintenance-report FAMILY` is read-only. Explicit `--maintenance` actions can update native packages, clean package caches, make local snapshots, select a native timezone or clean a bounded journal; irreversible effects are not presented as a transactional system rollback.

Enrollment refuses replacement of an existing manifest. Do not retire enrollment or change its trusted executables while a transaction is pending: rollback may need that owner state. Inspect transaction status and the [resource/risk matrix](SECURITY.md) before a separately reviewed ownership migration.
