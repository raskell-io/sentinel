//! Tests for `sni-certs` certificate folder scanning.
//!
//! A folder is a moving target: certificates are written into it by other
//! processes, sometimes non-atomically, and removed without the proxy being
//! told. The scanner is therefore built to skip individual problems loudly
//! rather than fail, so one half-written file cannot take every other
//! certificate on the listener down with it.
//!
//! Part of zentinelproxy/zentinel#117.

use std::fs;
use std::path::{Path, PathBuf};

use zentinel_config::{CertFolderReloadMode, SniCertFolder, TlsConfig};
use zentinel_proxy::tls::SniResolver;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/tls")
}

/// Copy a fixture pair into `dir` under a new stem.
fn place(dir: &Path, fixture_stem: &str, as_stem: &str) {
    fs::copy(
        fixtures().join(format!("{fixture_stem}.crt")),
        dir.join(format!("{as_stem}.crt")),
    )
    .expect("copy cert");
    fs::copy(
        fixtures().join(format!("{fixture_stem}.key")),
        dir.join(format!("{as_stem}.key")),
    )
    .expect("copy key");
}

fn config_with_folder(dir: &Path) -> TlsConfig {
    TlsConfig {
        cert_file: Some(fixtures().join("server-default.crt")),
        key_file: Some(fixtures().join("server-default.key")),
        additional_certs: vec![],
        cert_folders: vec![SniCertFolder {
            cert_folder: dir.to_path_buf(),
            reload_mode: CertFolderReloadMode::Off,
            reload_interval: std::time::Duration::from_secs(30),
        }],
        allow_sni_overlaps: false,
        ca_file: None,
        min_version: zentinel_common::types::TlsVersion::Tls12,
        max_version: None,
        cipher_suites: vec![],
        client_auth: false,
        ocsp_stapling: false,
        session_resumption: true,
        acme: None,
    }
}

/// Common name of a DER-encoded certificate.
fn cn_of(der: &[u8]) -> String {
    let (_, parsed) = x509_parser::parse_x509_certificate(der).expect("parseable certificate");
    let cn = parsed
        .subject()
        .iter_common_name()
        .next()
        .and_then(|cn| cn.as_str().ok())
        .unwrap_or("(no cn)")
        .to_string();
    cn
}

/// The certificate served for a hostname, identified by its CN.
fn resolved_cn(resolver: &SniResolver, server_name: &str) -> String {
    let cert = resolver.resolve(Some(server_name));
    let der = cert.cert.first().expect("certificate present").to_vec();
    cn_of(&der)
}

#[test]
fn certificates_in_a_folder_are_discovered_and_served() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");
    place(dir.path(), "server-secure", "secure");

    // Every fixture certificate carries `DNS:localhost`, so any two of them
    // collide on that name once hostnames are auto-extracted. That is the
    // realistic shape of a scanned folder — see the dedicated test below —
    // so overlaps are permitted here.
    let mut config = config_with_folder(dir.path());
    config.allow_sni_overlaps = true;
    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    // Hostnames come from each certificate's CN and SANs — no config lists them.
    assert_eq!(resolved_cn(&resolver, "api.example.com"), "api.example.com");
    assert_eq!(
        resolved_cn(&resolver, "secure.example.com"),
        "secure.example.com"
    );
    // Anything unmatched still falls back to the default certificate.
    assert_eq!(resolved_cn(&resolver, "unknown.test"), "example.com");
}

#[test]
fn an_empty_folder_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = config_with_folder(dir.path());
    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    assert_eq!(resolved_cn(&resolver, "anything"), "example.com");
}

#[test]
fn a_missing_folder_is_reported_but_does_not_fail_the_listener() {
    let mut config = config_with_folder(Path::new("/nonexistent/zentinel/certs"));
    config.additional_certs = vec![];

    let resolver = SniResolver::from_config(&config, Some("folder-test"))
        .expect("a missing folder must not stop the listener from starting");
    assert_eq!(resolved_cn(&resolver, "anything"), "example.com");
}

/// One unusable file must not cost the listener every other certificate.
#[test]
fn a_malformed_certificate_is_skipped_and_the_rest_still_load() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");

    // A half-written certificate, as a non-atomic copy would leave behind.
    fs::write(
        dir.path().join("broken.crt"),
        b"-----BEGIN CERTIFICATE-----\ntrunc",
    )
    .unwrap();
    fs::write(dir.path().join("broken.key"), b"not a key").unwrap();

    let config = config_with_folder(dir.path());
    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    assert_eq!(resolved_cn(&resolver, "api.example.com"), "api.example.com");
}

#[test]
fn a_certificate_without_a_key_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");
    fs::copy(
        fixtures().join("server-secure.crt"),
        dir.path().join("orphan.crt"),
    )
    .unwrap();

    let config = config_with_folder(dir.path());
    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    assert_eq!(resolved_cn(&resolver, "api.example.com"), "api.example.com");
    // The orphaned certificate contributed nothing, so its name falls back.
    assert_eq!(resolved_cn(&resolver, "secure.example.com"), "example.com");
}

#[test]
fn unrelated_files_in_the_folder_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");
    fs::write(dir.path().join("README.txt"), b"notes").unwrap();
    fs::write(dir.path().join("api.crt.bak"), b"backup").unwrap();
    fs::create_dir(dir.path().join("subdir")).unwrap();

    let config = config_with_folder(dir.path());
    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    assert_eq!(resolved_cn(&resolver, "api.example.com"), "api.example.com");
}

/// Two certificates claiming the same name is a configuration error by
/// default, because which one wins would otherwise be invisible.
#[test]
fn overlapping_hostnames_are_rejected_by_default() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "a-api");
    place(dir.path(), "server-api", "b-api");

    let config = config_with_folder(dir.path());
    let err = SniResolver::from_config(&config, Some("folder-test"))
        .expect_err("an overlap should be rejected");

    let message = err.to_string();
    assert!(
        message.contains("Ambiguous SNI configuration"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("allow-sni-overlaps"),
        "the error should name the escape hatch: {message}"
    );
}

/// A trap worth pinning: certificates issued for the same environment often
/// share a SAN — every fixture here carries `DNS:localhost`. With hostnames
/// auto-extracted from the certificate, two such files in one folder collide,
/// and with the strict default that is a hard error.
///
/// This is the shape operators will hit first, and the failure arrives when a
/// file is dropped into the folder rather than when the config is written. The
/// error names `allow-sni-overlaps` for exactly that reason.
#[test]
fn certificates_sharing_a_san_collide_under_the_strict_default() {
    let dir = tempfile::tempdir().unwrap();
    // Different hostnames, but both carry DNS:localhost.
    place(dir.path(), "server-api", "api");
    place(dir.path(), "server-secure", "secure");

    let config = config_with_folder(dir.path());
    let err = SniResolver::from_config(&config, Some("folder-test"))
        .expect_err("the shared SAN should be reported");

    let message = err.to_string();
    assert!(
        message.contains("localhost"),
        "the error should name the colliding hostname: {message}"
    );
    assert!(
        message.contains("allow-sni-overlaps"),
        "and the way out of it: {message}"
    );
}

/// With overlaps allowed, the winner is decided by sorted path order rather
/// than by whatever order the filesystem happened to return.
#[test]
fn allowed_overlaps_resolve_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "a-api");
    place(dir.path(), "server-api", "b-api");

    let mut config = config_with_folder(dir.path());
    config.allow_sni_overlaps = true;

    // Build repeatedly: the same certificate must win every time.
    let first = {
        let r = SniResolver::from_config(&config, Some("folder-test")).expect("should build");
        resolved_cn(&r, "api.example.com")
    };
    for _ in 0..5 {
        let r = SniResolver::from_config(&config, Some("folder-test")).expect("should build");
        assert_eq!(
            resolved_cn(&r, "api.example.com"),
            first,
            "the overlap winner must not vary between builds"
        );
    }
}

#[test]
fn explicit_sni_blocks_and_scanned_folders_coexist() {
    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");

    let mut config = config_with_folder(dir.path());
    config.additional_certs = vec![zentinel_config::SniCertificate {
        hostnames: vec!["secure.example.com".to_string()],
        priority_hostnames: vec![],
        cert_file: Some(fixtures().join("server-secure.crt")),
        key_file: Some(fixtures().join("server-secure.key")),
        acme: None,
    }];

    let resolver = SniResolver::from_config(&config, Some("folder-test")).expect("should build");

    assert_eq!(resolved_cn(&resolver, "api.example.com"), "api.example.com");
    assert_eq!(
        resolved_cn(&resolver, "secure.example.com"),
        "secure.example.com"
    );
}

/// A certificate added to the folder after startup is picked up by a reload,
/// which is the point of scanning a folder rather than listing certificates.
#[test]
fn a_reload_picks_up_a_newly_added_certificate() {
    use zentinel_proxy::tls::HotReloadableSniResolver;

    fn served_cn(resolver: &HotReloadableSniResolver, name: &str) -> String {
        let cert = resolver.resolve(Some(name));
        let der = cert.cert.first().expect("certificate present").to_vec();
        cn_of(&der)
    }

    let dir = tempfile::tempdir().unwrap();
    place(dir.path(), "server-api", "api");

    let mut config = config_with_folder(dir.path());
    // The certificate added mid-test shares `DNS:localhost` with the one
    // already present.
    config.allow_sni_overlaps = true;
    let resolver = HotReloadableSniResolver::from_config(config, "folder-test").expect("build");

    assert_eq!(
        served_cn(&resolver, "secure.example.com"),
        "example.com",
        "before the certificate exists, the default is served"
    );

    place(dir.path(), "server-secure", "secure");
    resolver.reload().expect("reload should succeed");

    assert_eq!(
        served_cn(&resolver, "secure.example.com"),
        "secure.example.com",
        "the reload should have discovered the new certificate"
    );
}

// ---------------------------------------------------------------------------
// Reload observability
//
// #117 asked for "full rescan + diff + atomic swap on reload". The rescan and
// the swap shipped; the diff did not, so a reload logged counts and nothing
// else. Counts cannot answer the question an operator actually has after an
// unattended `reload-mode "watch"` rescan -- "did anything change?" -- because
// the most common event, a renewal, leaves every count identical.
// ---------------------------------------------------------------------------

/// Write a self-signed cert/key pair for `cn` into `dir` under `stem`.
///
/// Each call produces a fresh key, so two calls with the same `cn` model a
/// renewal: same hostname, different certificate.
fn issue(dir: &Path, stem: &str, cn: &str) {
    let mut params =
        rcgen::CertificateParams::new(vec![cn.to_string()]).expect("valid subject alt name");
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, cn);
    params.distinguished_name = dn;

    let key = rcgen::KeyPair::generate().expect("keypair");
    let cert = params.self_signed(&key).expect("self-signed cert");

    fs::write(dir.join(format!("{stem}.crt")), cert.pem()).expect("write cert");
    fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).expect("write key");
}

fn served(dir: &Path) -> std::collections::BTreeMap<String, String> {
    let mut config = config_with_folder(dir);
    config.allow_sni_overlaps = true;
    SniResolver::from_config(&config, Some("folder-test"))
        .expect("should build")
        .served_certs()
}

#[test]
fn a_renewal_changes_the_fingerprint_for_an_unchanged_hostname() {
    let dir = tempfile::tempdir().unwrap();
    issue(dir.path(), "renewme", "renew.example.com");
    let before = served(dir.path());

    // The renewal an ACME client would perform: same name, same filename, new
    // certificate written over the old one.
    issue(dir.path(), "renewme", "renew.example.com");
    let after = served(dir.path());

    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "a renewal must not change the hostname set -- that is what makes it invisible to counts"
    );
    assert_ne!(
        before.get("renew.example.com"),
        after.get("renew.example.com"),
        "the fingerprint must change, or the reload diff cannot see a renewal"
    );
}

#[test]
fn an_untouched_folder_produces_an_identical_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    issue(dir.path(), "stable", "stable.example.com");

    assert_eq!(
        served(dir.path()),
        served(dir.path()),
        "rescanning an unchanged folder must not report a change"
    );
}

#[test]
fn adding_and_removing_certificates_moves_the_hostname_set() {
    let dir = tempfile::tempdir().unwrap();
    issue(dir.path(), "first", "first.example.com");
    let before = served(dir.path());

    fs::remove_file(dir.path().join("first.crt")).expect("remove cert");
    fs::remove_file(dir.path().join("first.key")).expect("remove key");
    issue(dir.path(), "second", "second.example.com");
    let after = served(dir.path());

    assert!(before.contains_key("first.example.com"));
    assert!(!after.contains_key("first.example.com"));
    assert!(after.contains_key("second.example.com"));
    // The 1-for-1 rotation from the issue: counts are equal on both sides.
    assert_eq!(before.len(), after.len());
}

/// The default certificate has no SNI hostname, so a silent rotation of it
/// would be exactly as invisible as any other.
#[test]
fn the_default_certificate_is_part_of_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    assert!(served(dir.path()).contains_key("<default>"));
}
