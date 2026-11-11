//! Read-only online source audit. Metadata is bounded; large payloads use HEAD.
//! This intentionally does not install keyrings or claim cryptographic trust.

use reinstall_rs::command::{CommandInput, CommandRunner, CommandSpec, OutputMode, SystemRunner};
use reinstall_rs::host::sources::{self, SourceError};

const METADATA_LIMIT: usize = 1_048_576;

pub fn audit() -> Result<(), String> {
    audit_with_runner(&mut SystemRunner)
}

pub fn audit_with_runner<R: CommandRunner>(runner: &mut R) -> Result<(), String> {
    println!("Official x86_64 source audit: bounded metadata GET and payload HEAD only.");
    println!("Reachability and parsed hashes do not verify payloads or OpenPGP signatures.");
    let results = [
        ("Alpine", audit_alpine(runner)),
        ("Debian", audit_debian(runner)),
        ("Arch", audit_arch(runner)),
        ("Gentoo", audit_gentoo(runner)),
    ];
    let mut failures = Vec::new();
    for (target, result) in results {
        match result {
            Ok(report) => println!("{target}: {report}"),
            Err(error) => {
                eprintln!("{target}: FAILED: {error}");
                failures.push(format!("{target}: {error}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "source audit failed without fallback: {}",
            failures.join("; ")
        ))
    }
}

fn audit_alpine<R: CommandRunner>(runner: &mut R) -> Result<String, String> {
    let metadata = get(runner, sources::ALPINE_RELEASES_URL)?;
    let source = sources::parse_alpine_netboot(&metadata, "x86_64").map_err(source_error)?;
    let checksums = get(runner, &source.checksum_url)?;
    sources::validate_alpine_checksum(&source, &checksums).map_err(source_error)?;
    head(runner, &source.url)?;
    head(runner, &source.signature_url)?;
    Ok(format!(
        "{} netboot archive reachable; metadata SHA256 matches its .sha256 sidecar: {}\n  {}\n  detached signature advertised/reachable, NOT verified; archive NOT downloaded or hashed.",
        source.version, source.sha256, source.url,
    ))
}

fn audit_debian<R: CommandRunner>(runner: &mut R) -> Result<String, String> {
    let metadata = get(runner, sources::DEBIAN_RELEASE_URL)?;
    let source = sources::parse_debian_archive(&metadata, "x86_64").map_err(source_error)?;
    let inrelease = get(runner, &source.inrelease_url)?;
    sources::validate_debian_inrelease(&source, &inrelease).map_err(source_error)?;
    head(runner, &source.security_inrelease_url)?;
    Ok(format!(
        "stable {} ({}) amd64 archive and signed-index references reachable; metadata identities match.\n  {}\n  InRelease armor parsed, OpenPGP signature NOT verified; debootstrap/APT must use trusted Debian archive keys. No Debian ISO is used.",
        source.version, source.codename, source.archive,
    ))
}

fn audit_arch<R: CommandRunner>(runner: &mut R) -> Result<String, String> {
    let source = sources::arch_repositories("x86_64").map_err(source_error)?;
    head(runner, &source.core_db)?;
    head(runner, &source.extra_db)?;
    Ok(format!(
        "core/extra x86_64 repository databases reachable.\n  {}\n  Required package policy: {}; trusted {} required. Package signatures NOT tested. No ISO/bootstrap tar is used by the pacstrap path.",
        sources::ARCH_MIRROR,
        source.signature_policy,
        source.keyring_package,
    ))
}

fn audit_gentoo<R: CommandRunner>(runner: &mut R) -> Result<String, String> {
    let index = get(runner, sources::GENTOO_STAGE3_INDEX_URL)?;
    let source = sources::parse_gentoo_stage3(&index, "x86_64").map_err(source_error)?;
    let digests = get(runner, &source.digests_url)?;
    let sha512 = sources::parse_gentoo_sha512(&source, &digests).map_err(source_error)?;
    head(runner, &source.url)?;
    head(runner, &source.signature_url)?;
    Ok(format!(
        "amd64 systemd stage3 {} reachable (index size {} bytes); selected SHA512 parsed: {}\n  {}\n  clearsigned index/DIGESTS parsed; signatures NOT verified; stage3 NOT downloaded or hashed.",
        source.timestamp, source.size, sha512, source.url,
    ))
}

fn source_error(error: SourceError) -> String {
    error.to_string()
}

fn get<R: CommandRunner>(runner: &mut R, url: &str) -> Result<String, String> {
    let bytes = request(runner, url, false)?;
    String::from_utf8(bytes).map_err(|_| format!("non-UTF-8 metadata from {url}"))
}

fn head<R: CommandRunner>(runner: &mut R, url: &str) -> Result<(), String> {
    request(runner, url, true).map(|_| ())
}

fn request<R: CommandRunner>(
    runner: &mut R,
    url: &str,
    headers_only: bool,
) -> Result<Vec<u8>, String> {
    let mut args = vec![
        "--disable",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "10",
        "--max-time",
        "30",
        "--max-redirs",
        "3",
        "-f",
        "-sS",
        "-L",
    ];
    if headers_only {
        args.push("--head");
    } else {
        args.extend(["--max-filesize", "1048576"]);
    }
    args.push(url);
    let command = CommandSpec::new("curl", args)
        .phase("audit-official-sources")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner
        .run(&command)
        .map_err(|error| format!("cannot fetch {url}: {error}"))?;
    if output.status != Some(0) {
        return Err(format!(
            "request failed for {url} (curl status {:?})",
            output.status
        ));
    }
    if output.stdout.len() > METADATA_LIMIT {
        return Err(format!("response exceeds the metadata limit for {url}"));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reinstall_rs::command::{CommandOutput, MockRunner};

    #[test]
    fn audit_transport_keeps_tls_limits_and_never_gets_payloads() {
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(0), b"metadata".to_vec(), vec![]),
            CommandOutput::new(Some(0), vec![], vec![]),
        ]);
        get(&mut runner, sources::ALPINE_RELEASES_URL).unwrap();
        head(&mut runner, "https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz").unwrap();
        for call in runner.calls() {
            assert_eq!(call.program, "curl");
            assert_eq!(call.args[0], "--disable");
            assert!(
                call.args
                    .windows(2)
                    .any(|pair| pair == ["--proto", "=https"])
            );
            assert!(
                call.args
                    .windows(2)
                    .any(|pair| pair == ["--proto-redir", "=https"])
            );
            assert!(
                call.args
                    .windows(2)
                    .any(|pair| pair == ["--max-time", "30"])
            );
            assert!(
                !call
                    .args
                    .iter()
                    .any(|arg| arg == "-k" || arg == "--insecure")
            );
        }
        assert!(
            runner.calls()[0]
                .args
                .iter()
                .any(|arg| arg == "--max-filesize")
        );
        assert!(runner.calls()[1].args.iter().any(|arg| arg == "--head"));
        assert!(
            !runner.calls()[1]
                .args
                .iter()
                .any(|arg| arg == "--max-filesize")
        );
    }

    #[test]
    fn request_rejects_failures_and_oversized_metadata() {
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(22), vec![], vec![]),
            CommandOutput::new(Some(0), vec![b'a'; METADATA_LIMIT + 1], vec![]),
        ]);
        assert!(get(&mut runner, sources::ALPINE_RELEASES_URL).is_err());
        assert!(get(&mut runner, sources::ALPINE_RELEASES_URL).is_err());
    }
}
