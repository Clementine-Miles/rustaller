# DMITBox local

A security-first refactor of the supplied standalone Bash toolbox. Development uses testable modules; distribution remains one local script. It provides no remote management server, telemetry, dashboard or downloaded-script installer.

High-risk changes use explicit resource ownership, durable transaction records and independent recovery. SSH and firewall changes require evidence from a newly authenticated connection or an identified console workflow. A configuration hash, valid syntax or local listener never proves external access.

Within rustaller, use `cargo xtask local-tool build` and `cargo xtask local-tool check`. These repository integration commands are implemented in Rust. The original standalone development fixtures below are retained for provenance and independent development. No installer or worker automatically invokes this component.

Build and inspect the artifact:

```sh
python3 build/build.py
bash dist/dmitbox-local.sh --help
```

Python is a development build tool; it is not a runtime dependency. Read-only commands do not install packages or run unsolicited probes. Install required native dependencies explicitly: Bash, jq, GNU coreutils, util-linux and iproute2 form the base; file metadata verification needs native ACL/attr tools. Individual adapters require their native manager, nftables, OpenSSH or explicit probe tools. Review the [ownership and risk matrix](docs/SECURITY.md) and [supported boundaries](docs/LIMITATIONS.md) before making changes.

For privileged use, verify the distribution checksum and install the reviewed artifact in a root-owned path with trusted ancestors. A user-writable development checkout cannot serve as a privileged recovery worker.

```sh
(cd dist && sha256sum -c SHA256SUMS)
sudo install -o root -g root -m 0700 dist/dmitbox-local.sh /usr/local/sbin/dmitbox-local
```

Inspect local state and prepare a change:

```sh
bash dist/dmitbox-local.sh --net-report
bash dist/dmitbox-local.sh --plan mtu eth0 1400 runtime > mtu-plan.json
bash dist/dmitbox-local.sh --diff mtu-plan.json
```

The example does not select an interface for you or apply the change. DNS plans use `--plan dns INTERFACE cloudflare persistent`, or `--plan dns INTERFACE custom persistent ADDRESS ...` with one numeric address per argument. Persistent DNS is the default. Runtime overrides must be requested explicitly.

Privileged application uses `--apply-plan PLAN_JSON`. Keep a separate authenticated access path available, retain the displayed one-use challenge privately, and inspect `--net-status TXID`. Confirmation uses `--net-confirm TXID ssh` from a fresh authenticated connection, or the separately enrolled console workflow. Rollback uses `--net-rollback TXID`. SSH port retirement requires another fresh authentication before the original deadline.

Existing `--net-*` CLI names remain recognized. An unsupported owner or recovery case returns an error; no legacy unsafe fallback is invoked. Some former mutations, including generic link up/down, socket-activated SSH, active Fail2Ban coordination and Swap resizing, remain unsupported. The interactive menu presents reports and plans, with application remaining an explicit action. `--dns-probe` and `--pmtu-probe` are explicitly requested network probes. See [enrollment and configuration examples](docs/CONFIGURATION.md) for each adapter's prerequisites.

Run development checks:

```sh
bash tests/run.sh --integration
bash tests/run.sh --strict-integration
```

The runner writes reproducible local logs and a JSON report under `artifacts/`. A skip is not a pass. Native namespace and minimal QEMU init/SSH fixtures prove their named assertions; the full Debian/Arch/Alpine manager matrix and real delegated-prefix/external-NAT66 tests remain separate acceptance requirements. This is a development release until those requirements pass. See [verification levels and required scenarios](docs/TESTING.md), [prioritized changes](docs/CHANGELOG.md), and [unsupported or unverified behavior](docs/LIMITATIONS.md).

The [recorded results](docs/TEST-EVIDENCE.md) and [native recovery evidence](docs/NATIVE-EVIDENCE.md) identify the tested artifact and each verification boundary.

The supplied r3 script and review are archived unchanged under `upstream/` for comparison. Legacy unsafe mutation paths and environment bypass flags are not executable fallbacks in the new distribution.

The refactor is distributed under [GPLv3](LICENSE). Original upstream declarations remain in the archived source; [provenance](docs/UPSTREAM.md) identifies those inputs.
