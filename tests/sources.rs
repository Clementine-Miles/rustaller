use rustaller::host::sources::{
    ALPINE_RELEASES_URL, ARCH_MIRROR, DEBIAN_RELEASE_URL, GENTOO_STAGE3_INDEX_URL, SourceKind,
    arch_repositories, clearsigned_payload, parse_alpine_netboot, parse_debian_archive,
    parse_gentoo_sha512, parse_gentoo_stage3, source_kind, validate_alpine_checksum,
    validate_debian_inrelease,
};
use rustaller::model::Target;

const SHA256: &str = "0228553464c9588ffeb63ec04d26abb5f79405006b74574034394199af47dab4";
const ALPINE: &str = "---\n-\n  version: 3.24.3\n  flavor: alpine-minirootfs\n-\n  branch: v3.24\n  arch: x86_64\n  version: 3.24.3\n  flavor: alpine-netboot\n  file: alpine-netboot-3.24.3-x86_64.tar.gz\n  iso: alpine-netboot-3.24.3-x86_64.tar.gz\n  sha256: 0228553464c9588ffeb63ec04d26abb5f79405006b74574034394199af47dab4\n";
const DEBIAN: &str = "Origin: Debian\nSuite: stable\nVersion: 13.7\nCodename: trixie\nArchitectures: all amd64 arm64\n";
const STAGE3_PATH: &str = "20261018T124555Z/stage3-amd64-systemd-20261018T124555Z.tar.xz";
const STAGE3_FILENAME: &str = "stage3-amd64-systemd-20261018T124555Z.tar.xz";

// Deliberately not a cryptographic signature: these parsers only inspect armor.
fn armored(body: &str) -> String {
    format!(
        "-----BEGIN PGP SIGNED MESSAGE-----\nHash: SHA256\n\n{body}\n-----BEGIN PGP SIGNATURE-----\n\nfixture-signature-not-authenticated\n-----END PGP SIGNATURE-----\n"
    )
}

#[test]
fn catalog_distinguishes_actual_installation_resources_and_rejects_other_architectures() {
    for (target, kind) in [
        (Target::Alpine, SourceKind::NetbootArchive),
        (Target::Debian, SourceKind::DebianArchive),
        (Target::Arch, SourceKind::PackageRepository),
        (Target::Gentoo, SourceKind::SystemdStage3),
    ] {
        assert_eq!(source_kind(target, "x86_64").unwrap(), kind);
        assert!(source_kind(target, "aarch64").is_err());
    }
    for target in [Target::LocalIso, Target::NetbootXyz, Target::Reset] {
        assert!(source_kind(target, "x86_64").is_err());
    }
    for url in [
        ALPINE_RELEASES_URL,
        DEBIAN_RELEASE_URL,
        ARCH_MIRROR,
        GENTOO_STAGE3_INDEX_URL,
    ] {
        assert!(url.starts_with("https://"));
    }
}

#[test]
fn alpine_selects_exact_versioned_netboot_archive_and_matches_real_checksum_sidecar() {
    let source = parse_alpine_netboot(ALPINE, "x86_64").unwrap();
    assert_eq!(source.version, "3.24.3");
    assert_eq!(source.branch, "v3.24");
    assert_eq!(source.sha256, SHA256);
    assert_eq!(
        source.url,
        "https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz"
    );
    assert_eq!(source.checksum_url, format!("{}.sha256", source.url));
    assert_eq!(source.signature_url, format!("{}.asc", source.url));
    validate_alpine_checksum(&source, &format!("{SHA256}  {}\n", source.filename)).unwrap();
    validate_alpine_checksum(&source, &format!("{SHA256} *{}\n", source.filename)).unwrap();
    assert!(!source.checksum_url.contains("vmlinuz"));
}

#[test]
fn alpine_rejects_ambiguous_identity_unsafe_names_and_missing_or_invalid_hashes() {
    for metadata in [
        ALPINE.replace("arch: x86_64", "arch: aarch64"),
        ALPINE.replace("branch: v3.24", "branch: v3.25"),
        ALPINE.replace("file: alpine-netboot", "file: ../alpine-netboot"),
        ALPINE.replace("flavor: alpine-netboot", "flavor: alpine-standard"),
        ALPINE.replace(SHA256, "not-a-checksum"),
        ALPINE.replace("  sha256:", "  arch: x86_64\n  sha256:"),
        format!("{ALPINE}{ALPINE}"),
    ] {
        assert!(
            parse_alpine_netboot(&metadata, "x86_64").is_err(),
            "accepted {metadata}"
        );
    }
    assert!(parse_alpine_netboot(ALPINE, "amd64").is_err());
    let source = parse_alpine_netboot(ALPINE, "x86_64").unwrap();
    for checksums in [
        format!("{}  {}\n", "f".repeat(64), source.filename),
        format!("{SHA256}  wrong.tar.gz\n"),
        format!(
            "{SHA256}  {}\n{SHA256}  {}\n",
            source.filename, source.filename
        ),
        format!("{SHA256} **{}\n", source.filename),
    ] {
        assert!(validate_alpine_checksum(&source, &checksums).is_err());
    }
}

#[test]
fn debian_pins_signed_index_to_validated_stable_codename_and_amd64() {
    let source = parse_debian_archive(DEBIAN, "x86_64").unwrap();
    assert_eq!(source.version, "13");
    assert_eq!(source.codename, "trixie");
    assert_eq!(
        source.inrelease_url,
        "https://deb.debian.org/debian/dists/trixie/InRelease"
    );
    assert_eq!(
        source.security_inrelease_url,
        "https://security.debian.org/debian-security/dists/trixie-security/InRelease"
    );
    validate_debian_inrelease(&source, &armored(DEBIAN)).unwrap();
    assert!(
        validate_debian_inrelease(&source, &armored(&DEBIAN.replace("trixie", "forky"))).is_err()
    );
    for metadata in [
        DEBIAN.replace("Origin: Debian", "Origin: Untrusted"),
        DEBIAN.replace("Suite: stable", "Suite: testing"),
        DEBIAN.replace("amd64", "i386"),
        DEBIAN.replace("amd64", "amd64 amd64"),
        DEBIAN.replace("trixie", "../trixie"),
        format!("{DEBIAN}Origin: Debian\n"),
        format!("{DEBIAN}Version:\t99\n"),
        format!("{DEBIAN}Codename:\tforky\n"),
    ] {
        assert!(parse_debian_archive(&metadata, "x86_64").is_err());
    }
    assert!(parse_debian_archive(DEBIAN, "aarch64").is_err());
}

#[test]
fn arch_uses_package_repositories_and_requires_package_signatures() {
    let source = arch_repositories("x86_64").unwrap();
    assert_eq!(
        source.core_db,
        format!("{ARCH_MIRROR}/core/os/x86_64/core.db")
    );
    assert_eq!(
        source.extra_db,
        format!("{ARCH_MIRROR}/extra/os/x86_64/extra.db")
    );
    assert_eq!(source.keyring_package, "archlinux-keyring");
    assert_eq!(source.signature_policy, "Required DatabaseOptional");
    assert!(arch_repositories("aarch64").is_err());
}

#[test]
fn gentoo_selects_systemd_stage3_and_sha512_for_the_exact_archive() {
    let source = parse_gentoo_stage3(
        &armored(&format!("# latest\n{STAGE3_PATH} 297255400")),
        "x86_64",
    )
    .unwrap();
    assert_eq!(source.timestamp, "20261018T124555Z");
    assert_eq!(source.size, 297255400);
    assert_eq!(source.filename, STAGE3_FILENAME);
    assert_eq!(
        source.url,
        format!("https://distfiles.gentoo.org/releases/amd64/autobuilds/{STAGE3_PATH}")
    );
    assert_eq!(source.digests_url, format!("{}.DIGESTS", source.url));
    assert_eq!(source.signature_url, format!("{}.asc", source.url));
    let sha512 = "b".repeat(128);
    let digests = armored(&format!(
        "# BLAKE2B HASH\n{}  {STAGE3_FILENAME}\n# SHA512 HASH\n{sha512}  {STAGE3_FILENAME}\n# SHA512 HASH\n{}  {STAGE3_FILENAME}.CONTENTS.gz",
        "a".repeat(128),
        "c".repeat(128)
    ));
    assert_eq!(parse_gentoo_sha512(&source, &digests).unwrap(), sha512);
}

#[test]
fn gentoo_rejects_unexpected_or_duplicate_stage3_records() {
    for record in [
        format!("{STAGE3_PATH} 297255400\n{STAGE3_PATH} 297255400"),
        format!("../{STAGE3_PATH} 297255400"),
        format!("https://untrusted.example/{STAGE3_PATH} 297255400"),
        format!("{} 297255400", STAGE3_PATH.replace("amd64", "arm64")),
        format!("{} 297255400", STAGE3_PATH.replace("systemd", "openrc")),
        format!(
            "{} 297255400",
            STAGE3_PATH.replacen("20261018", "../..foo", 1)
        ),
        format!("{STAGE3_PATH} 0"),
        format!("{STAGE3_PATH} -1"),
    ] {
        assert!(
            parse_gentoo_stage3(&armored(&record), "x86_64").is_err(),
            "accepted {record}"
        );
    }
    assert!(parse_gentoo_stage3(&format!("{STAGE3_PATH} 297255400"), "x86_64").is_err());
    assert!(parse_gentoo_stage3(&armored(&format!("{STAGE3_PATH} 297255400")), "aarch64").is_err());
}

#[test]
fn gentoo_digest_requires_one_correctly_named_sha512_in_signed_metadata() {
    let source =
        parse_gentoo_stage3(&armored(&format!("{STAGE3_PATH} 297255400")), "x86_64").unwrap();
    let good = format!("# SHA512 HASH\n{}  {STAGE3_FILENAME}", "b".repeat(128));
    for body in [
        format!("{good}\n{good}"),
        good.replace(STAGE3_FILENAME, "other.tar.xz"),
        good.replace(STAGE3_FILENAME, "../stage3.tar.xz"),
        good.replace(&"b".repeat(128), "short"),
        good.replace("# SHA512 HASH\n", ""),
        good.replace("SHA512", "SHA256"),
        good.replace(STAGE3_FILENAME, &format!("{STAGE3_FILENAME}.CONTENTS.gz")),
    ] {
        assert!(parse_gentoo_sha512(&source, &armored(&body)).is_err());
    }
    assert!(parse_gentoo_sha512(&source, &good).is_err());
}

#[test]
fn signed_armor_checks_structure_without_claiming_to_authenticate_signatures() {
    let valid = armored("payload");
    assert_eq!(clearsigned_payload(&valid).unwrap(), "payload\n");
    assert!(clearsigned_payload(&valid.replace('\n', "\r\n")).is_ok());
    for invalid in [
        "payload".into(),
        valid.replace("Hash: SHA256\n", ""),
        valid.replace("-----END PGP SIGNATURE-----", ""),
        valid.replace("fixture-signature-not-authenticated", ""),
        format!("{valid}{valid}"),
    ] {
        assert!(clearsigned_payload(&invalid).is_err());
    }
}
