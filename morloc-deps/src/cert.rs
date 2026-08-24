//! Pure certificate-bundle parsing, classification, and normalization.
//!
//! Used to support corporate TLS-inspection firewalls: a user supplies a CA
//! bundle and the manager teaches pixi/rattler/uv (and, in containers, apt and
//! the language installers) to trust it. This module is the trust-free core: it
//! takes bytes and produces either a validated report or a precise verdict. It
//! performs no filesystem discovery and no network access; the caller supplies
//! the bytes.
//!
//! Safety model: a private key must never be baked into an image. The guarantee
//! is structural -- `parse_bundle` keeps only blocks that decode as X.509 certs
//! and `normalize_to_pem` re-encodes *those*, so a key (disjoint ASN.1, fails
//! `X509Certificate::from_der`) can never be re-emitted. The `KeyMaterial`
//! refusal is a UX signal on top of that guarantee, not the guarantee itself.

use std::fmt;

use pem::{EncodeConfig, LineEnding, Pem};
use sha2::{Digest, Sha256};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::error::DepsError;

/// Bundles larger than this are rejected before parsing. A CA bundle is a
/// handful of certificates; anything past a megabyte is the wrong file.
pub const DEFAULT_SIZE_CAP: u64 = 1 << 20;

/// The portable set of environment-variable names pointed at a trusted CA
/// bundle. Different layers read different variables, so all are set: rustls /
/// rattler + curl (`SSL_CERT_FILE`, `CURL_CA_BUNDLE`), pip / requests
/// (`REQUESTS_CA_BUNDLE`), conda (`CONDA_SSL_VERIFY`), Node
/// (`NODE_EXTRA_CA_CERTS`), and git (`GIT_SSL_CAINFO`, process-scoped -- never
/// `git config --global`).
pub const CERT_ENV_VARS: &[&str] = &[
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "CONDA_SSL_VERIFY",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
];

/// `(name, value)` pairs setting every [`CERT_ENV_VARS`] entry to `bundle`.
pub fn cert_env_pairs(bundle: &str) -> Vec<(String, String)> {
    CERT_ENV_VARS
        .iter()
        .map(|k| (k.to_string(), bundle.to_string()))
        .collect()
}

/// PEM label for a normalized certificate block.
const CERT_LABEL: &str = "CERTIFICATE";

/// An unambiguous problem with the supplied bytes. These map to hard errors /
/// refusals at the call site; anything ambiguous is reported as a neutral fact
/// on `CertFacts` instead.
#[derive(Debug, PartialEq, Eq)]
pub enum CertError {
    /// The file was empty or contained only whitespace.
    Empty,
    /// The file exceeded the size cap (enforced by the caller before reading).
    TooLarge { size: u64, cap: u64 },
    /// A private-key (or PKCS#7/PKCS#12) block was present. Refused outright: a
    /// trust bundle is public certificates only.
    KeyMaterial { label: String },
    /// The bytes were raw DER but not an X.509 certificate (e.g. a DER-encoded
    /// private key or a PKCS#12 archive).
    NonCertDer,
    /// The bytes look like text rather than a certificate -- most often a proxy
    /// / captive-portal HTML error page saved with a `.pem` extension.
    LooksLikeText { snippet: String },
    /// PEM blocks were present but none decoded as a certificate.
    NoCerts,
}

impl fmt::Display for CertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertError::Empty => write!(f, "the certificate file is empty"),
            CertError::TooLarge { size, cap } => write!(
                f,
                "the certificate file is {size} bytes, larger than the {cap}-byte limit; \
                 a CA bundle is a handful of certificates -- this is probably the wrong file"
            ),
            CertError::KeyMaterial { label } => write!(
                f,
                "the file contains private key material (a '{label}' block); a trust bundle \
                 is public certificates only -- export just the CA certificate, not the key"
            ),
            CertError::NonCertDer => write!(
                f,
                "the file is DER-encoded but is not an X.509 certificate (it looks like a key \
                 or a PKCS#12 archive); supply the CA certificate in PEM or DER form"
            ),
            CertError::LooksLikeText { snippet } => write!(
                f,
                "the file is text, not a certificate -- it may be a proxy error page saved as \
                 a certificate. First bytes: {snippet:?}"
            ),
            CertError::NoCerts => write!(
                f,
                "no certificates were found in the file (PEM blocks were present but none \
                 decoded as an X.509 certificate)"
            ),
        }
    }
}

impl std::error::Error for CertError {}

impl From<CertError> for DepsError {
    fn from(e: CertError) -> Self {
        DepsError::Env(e.to_string())
    }
}

/// Neutral, human-readable facts about one certificate. Fields that cannot be
/// determined unambiguously (e.g. `is_ca` for a v1 certificate with no
/// BasicConstraints extension) are left as `None` rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFacts {
    pub subject: String,
    pub subject_cn: Option<String>,
    pub subject_o: Option<String>,
    pub issuer: String,
    pub issuer_cn: Option<String>,
    pub issuer_o: Option<String>,
    pub not_before: String,
    pub not_after: String,
    pub not_before_ts: Option<i64>,
    pub not_after_ts: Option<i64>,
    /// Subject and issuer names are byte-identical (a root). A neutral fact.
    pub is_self_signed: bool,
    /// `Some(true/false)` from BasicConstraints; `None` when the extension is
    /// absent (ambiguous -- stays a fact, never a verdict).
    pub is_ca: Option<bool>,
    pub serial_hex: String,
    /// Human certificate version (1, 2, or 3).
    pub version: u8,
    /// SHA-256 of the canonical DER, colon-separated uppercase hex.
    pub sha256_fingerprint: String,
}

/// A certificate that positively decoded as X.509. `der` holds its canonical
/// DER bytes; it is the only material re-emitted by `normalize_to_pem`.
#[derive(Debug, Clone)]
pub struct ValidatedCert {
    pub facts: CertFacts,
    der: Vec<u8>,
}

/// A block that was present but not re-emitted (failed to decode, or an
/// unrecognized label). Reported so nothing is silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedBlock {
    pub index: usize,
    pub label: String,
    pub reason: String,
}

/// The result of parsing a bundle: the certificates that will be trusted, plus
/// any blocks that were reported-but-excluded.
#[derive(Debug, Clone)]
pub struct CertReport {
    pub certs: Vec<ValidatedCert>,
    pub excluded: Vec<ExcludedBlock>,
}

/// Parse, classify, and validate a certificate bundle. Returns a verdict
/// (`CertError`) for unambiguous problems, otherwise a `CertReport` of the
/// certificates that survived validation.
pub fn parse_bundle(bytes: &[u8]) -> Result<CertReport, CertError> {
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Err(CertError::Empty);
    }

    // `parse_many` finds every armored block; a file with no PEM armor (raw
    // DER, or junk) yields an empty vector, which routes to the raw-DER path.
    let pems = pem::parse_many(bytes).unwrap_or_default();
    if pems.is_empty() {
        return parse_raw_der(bytes);
    }

    let mut certs = Vec::new();
    let mut excluded = Vec::new();
    for (index, block) in pems.iter().enumerate() {
        let label = block.tag().to_string();
        if is_key_label(&label) {
            return Err(CertError::KeyMaterial { label });
        }
        if is_cert_label(&label) {
            match validate_cert(block.contents()) {
                Ok(vc) => certs.push(vc),
                Err(reason) => excluded.push(ExcludedBlock { index, label, reason }),
            }
        } else {
            excluded.push(ExcludedBlock {
                index,
                label,
                reason: "unrecognized PEM label".to_string(),
            });
        }
    }

    if certs.is_empty() {
        return Err(CertError::NoCerts);
    }
    Ok(CertReport { certs, excluded })
}

/// Canonical PEM of the validated certificates only (corp-only). For the
/// container `COPY` -- `update-ca-certificates` appends it onto Debian's roots.
pub fn normalize_to_pem(report: &CertReport) -> Vec<u8> {
    let pems: Vec<Pem> = report
        .certs
        .iter()
        .map(|vc| Pem::new(CERT_LABEL, vc.der.clone()))
        .collect();
    let cfg = EncodeConfig::new().set_line_ending(LineEnding::LF);
    pem::encode_many_config(&pems, cfg).into_bytes()
}

/// Canonical corp PEM followed by the vendored public roots. For the host
/// `SSL_CERT_FILE`, where rustls *replaces* the trust store: a corp-only file
/// would break public HTTPS, so the public roots are concatenated in.
pub fn host_bundle_with_roots(report: &CertReport) -> Vec<u8> {
    let mut out = normalize_to_pem(report);
    out.extend_from_slice(public_roots_pem());
    out
}

/// The vendored Mozilla/curl public-root bundle (full certificates in PEM).
/// Provenance and date are recorded in the file's own header.
pub fn public_roots_pem() -> &'static [u8] {
    include_bytes!("../assets/cacert.pem")
}

/// SHA-256 fingerprints of the validated certificates, sorted. Used as a
/// rebuild cache-key fragment and for drift detection.
pub fn fingerprints(report: &CertReport) -> Vec<String> {
    let mut fps: Vec<String> = report
        .certs
        .iter()
        .map(|vc| vc.facts.sha256_fingerprint.clone())
        .collect();
    fps.sort();
    fps
}

// --- internals ---------------------------------------------------------------

fn is_key_label(label: &str) -> bool {
    let upper = label.to_ascii_uppercase();
    upper.contains("PRIVATE KEY")
        || upper.contains("OPENSSH")
        || upper.contains("PKCS7")
        || upper.contains("PKCS12")
        || upper.contains("ENCRYPTED")
}

fn is_cert_label(label: &str) -> bool {
    matches!(
        label.to_ascii_uppercase().as_str(),
        "CERTIFICATE" | "TRUSTED CERTIFICATE" | "X509 CERTIFICATE"
    )
}

fn validate_cert(der: &[u8]) -> Result<ValidatedCert, String> {
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|e| format!("not a valid X.509 certificate: {e}"))?;
    Ok(ValidatedCert { facts: extract_facts(&cert, der), der: der.to_vec() })
}

fn parse_raw_der(bytes: &[u8]) -> Result<CertReport, CertError> {
    // Raw DER (no PEM armor): decode as many concatenated certificates as parse,
    // so a multi-certificate DER bundle is not silently reduced to its first
    // entry. Trailing non-certificate bytes (e.g. a newline) end the loop and are
    // ignored, mirroring the leniency of the PEM path.
    let mut certs = Vec::new();
    let mut rest = bytes;
    while let Ok((tail, cert)) = X509Certificate::from_der(rest) {
        if tail.len() >= rest.len() {
            break; // no progress; guard against a zero-length parse
        }
        let der = &rest[..rest.len() - tail.len()];
        certs.push(ValidatedCert { facts: extract_facts(&cert, der), der: der.to_vec() });
        rest = tail;
    }
    if !certs.is_empty() {
        return Ok(CertReport { certs, excluded: Vec::new() });
    }
    // Not a certificate. Distinguish "binary DER that isn't a cert" (a key or
    // PKCS#12) from "text that isn't a certificate at all" (a proxy HTML page).
    if bytes.first() == Some(&0x30) {
        Err(CertError::NonCertDer)
    } else {
        Err(CertError::LooksLikeText { snippet: text_snippet(bytes) })
    }
}

fn text_snippet(bytes: &[u8]) -> String {
    let n = bytes.len().min(48);
    String::from_utf8_lossy(&bytes[..n]).trim().to_string()
}

fn extract_facts(cert: &X509Certificate, der: &[u8]) -> CertFacts {
    let subject = cert.subject();
    let issuer = cert.issuer();
    let validity = cert.validity();
    CertFacts {
        subject: subject.to_string(),
        subject_cn: first_attr(subject.iter_common_name()),
        subject_o: first_attr(subject.iter_organization()),
        issuer: issuer.to_string(),
        issuer_cn: first_attr(issuer.iter_common_name()),
        issuer_o: first_attr(issuer.iter_organization()),
        not_before: validity.not_before.to_string(),
        not_after: validity.not_after.to_string(),
        not_before_ts: Some(validity.not_before.timestamp()),
        not_after_ts: Some(validity.not_after.timestamp()),
        is_self_signed: subject.as_raw() == issuer.as_raw(),
        is_ca: basic_constraints_ca(cert),
        serial_hex: cert.raw_serial_as_string(),
        version: (cert.version().0.saturating_add(1)) as u8,
        sha256_fingerprint: fingerprint(der),
    }
}

/// `Some(ca)` from BasicConstraints; `None` when the extension is absent or
/// malformed -- an unknowable, so left as a neutral fact rather than guessed.
fn basic_constraints_ca(cert: &X509Certificate) -> Option<bool> {
    match cert.basic_constraints() {
        Ok(Some(ext)) => Some(ext.value.ca),
        _ => None,
    }
}

/// First attribute rendered as a string, degrading to `#<hex>` for name
/// encodings (BMPString, TeletexString) that are not valid UTF-8 text -- never
/// panics on real-world corporate DNs.
fn first_attr<'a>(
    mut attrs: impl Iterator<Item = &'a x509_parser::x509::AttributeTypeAndValue<'a>>,
) -> Option<String> {
    let attr = attrs.next()?;
    match attr.as_str() {
        Ok(s) => Some(s.to_string()),
        Err(_) => Some(format!("#{}", hex_lower(attr.as_slice()))),
    }
}

fn fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let mut out = String::with_capacity(digest.len() * 3);
    for (i, b) in digest.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real, self-signed root CA certificate (extracted from the vendored
    // public-root bundle) -- known-good X.509 with BasicConstraints CA:TRUE.
    fn sample_cert_pem() -> String {
        let pems = pem::parse_many(public_roots_pem()).unwrap();
        let cfg = EncodeConfig::new().set_line_ending(LineEnding::LF);
        pem::encode_config(&Pem::new(CERT_LABEL, pems[0].contents().to_vec()), cfg)
    }

    #[test]
    fn multi_cert_roots_all_ca_and_self_signed() {
        let report = parse_bundle(public_roots_pem()).unwrap();
        assert!(report.certs.len() > 100, "vendored bundle should hold many roots");
        for vc in &report.certs {
            assert_eq!(vc.facts.is_ca, Some(true), "a public root is a CA");
            assert!(vc.facts.is_self_signed, "a public root is self-signed");
            assert!(vc.facts.not_after_ts.is_some());
        }
    }

    #[test]
    fn normalize_round_trips_block_count() {
        let report = parse_bundle(public_roots_pem()).unwrap();
        let out = normalize_to_pem(&report);
        let reparsed = pem::parse_many(&out).unwrap();
        assert_eq!(reparsed.len(), report.certs.len());
    }

    #[test]
    fn single_corp_ca() {
        let report = parse_bundle(sample_cert_pem().as_bytes()).unwrap();
        assert_eq!(report.certs.len(), 1);
        assert!(report.certs[0].facts.sha256_fingerprint.contains(':'));
    }

    #[test]
    fn mixed_cert_and_key_is_refused() {
        let mut input = sample_cert_pem();
        input.push_str(
            "-----BEGIN PRIVATE KEY-----\nMIIBVAIBADANBg==\n-----END PRIVATE KEY-----\n",
        );
        assert!(matches!(
            parse_bundle(input.as_bytes()),
            Err(CertError::KeyMaterial { .. })
        ));
    }

    #[test]
    fn raw_der_certificate() {
        let pems = pem::parse_many(public_roots_pem()).unwrap();
        let der = pems[0].contents();
        let report = parse_bundle(der).unwrap();
        assert_eq!(report.certs.len(), 1);
    }

    #[test]
    fn raw_der_multiple_certificates() {
        let pems = pem::parse_many(public_roots_pem()).unwrap();
        // Two certificates concatenated as raw DER, no PEM armor.
        let mut der = pems[0].contents().to_vec();
        der.extend_from_slice(pems[1].contents());
        let report = parse_bundle(&der).unwrap();
        assert_eq!(report.certs.len(), 2, "both concatenated DER certs kept");
    }

    #[test]
    fn raw_der_key_is_non_cert() {
        // A DER SEQUENCE that is not a certificate.
        let der = [0x30u8, 0x03, 0x02, 0x01, 0x00];
        assert!(matches!(parse_bundle(&der), Err(CertError::NonCertDer)));
    }

    #[test]
    fn html_page_looks_like_text() {
        let html = b"<!DOCTYPE html>\n<html><body>Proxy authentication required</body></html>";
        assert!(matches!(
            parse_bundle(html),
            Err(CertError::LooksLikeText { .. })
        ));
    }

    #[test]
    fn empty_file() {
        assert!(matches!(parse_bundle(b""), Err(CertError::Empty)));
        assert!(matches!(parse_bundle(b"   \n\t "), Err(CertError::Empty)));
    }

    #[test]
    fn junk_between_blocks_is_tolerated() {
        let cert = sample_cert_pem();
        let input = format!("leading junk\n{cert}\nmore junk between\n{cert}\ntrailing\n");
        let report = parse_bundle(input.as_bytes()).unwrap();
        assert_eq!(report.certs.len(), 2);
    }

    #[test]
    fn corrupt_cert_block_is_excluded_not_fatal() {
        let mut input = sample_cert_pem();
        // "foo" base64 -- valid PEM framing, invalid certificate DER.
        input.push_str("-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n");
        let report = parse_bundle(input.as_bytes()).unwrap();
        assert_eq!(report.certs.len(), 1);
        assert_eq!(report.excluded.len(), 1);
    }

    #[test]
    fn host_bundle_appends_public_roots() {
        let report = parse_bundle(sample_cert_pem().as_bytes()).unwrap();
        let corp = normalize_to_pem(&report);
        let host = host_bundle_with_roots(&report);
        assert!(host.starts_with(&corp), "corp certs come first");
        assert!(host.ends_with(public_roots_pem()), "public roots are appended");
    }

    #[test]
    fn fingerprints_are_sorted_and_present() {
        let report = parse_bundle(public_roots_pem()).unwrap();
        let fps = fingerprints(&report);
        assert_eq!(fps.len(), report.certs.len());
        let mut sorted = fps.clone();
        sorted.sort();
        assert_eq!(fps, sorted);
    }
}
