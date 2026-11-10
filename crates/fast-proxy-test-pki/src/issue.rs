use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};

/// A self-made certificate authority.
pub struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    /// The CA certificate, PEM.
    pub pem: String,
}

/// What to issue.
#[derive(Debug, Clone)]
pub struct LeafSpec {
    names: Vec<String>,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
}

impl LeafSpec {
    /// A certificate valid for the last day and the next 90 days, for `names`
    /// (DNS names, wildcards, or IP addresses).
    pub fn new(names: &[&str]) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            names: names.iter().map(ToString::to_string).collect(),
            not_before: now - Duration::days(1),
            not_after: now + Duration::days(90),
        }
    }

    /// Valid until `days` from now (negative: already expired).
    pub fn expiring_in_days(mut self, days: i64) -> Self {
        self.not_after = OffsetDateTime::now_utc() + Duration::days(days);
        if self.not_after < self.not_before {
            self.not_before = self.not_after - Duration::days(30);
        }
        self
    }

    /// Valid only from `days` in the future.
    pub fn starting_in_days(mut self, days: i64) -> Self {
        self.not_before = OffsetDateTime::now_utc() + Duration::days(days);
        self.not_after = self.not_before + Duration::days(30);
        self
    }
}

/// An issued leaf, as the PEM strings a user would upload.
#[derive(Debug, Clone)]
pub struct Issued {
    /// The leaf certificate alone.
    pub cert_pem: String,
    /// The leaf followed by the issuing CA.
    pub chain_pem: String,
    /// The private key as PKCS#8.
    pub key_pem: String,
    /// The issuing CA certificate.
    pub ca_pem: String,
}

impl TestCa {
    /// A new CA named `name`.
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key = KeyPair::generate().expect("generating a test CA key");
        let certificate = params.self_signed(&key).expect("self-signing the test CA");
        Self {
            pem: certificate.pem(),
            issuer: Issuer::new(params, key),
        }
    }

    /// Issues a leaf with a fresh P-256 key.
    pub fn issue(&self, spec: &LeafSpec) -> Issued {
        let key = KeyPair::generate().expect("generating a test leaf key");
        self.issue_for_key(spec, &key)
    }

    fn issue_for_key(&self, spec: &LeafSpec, key: &KeyPair) -> Issued {
        let mut params = CertificateParams::default();
        params.distinguished_name.push(
            DnType::CommonName,
            spec.names.first().map_or("test", String::as_str),
        );
        params.subject_alt_names = spec.names.iter().map(|name| san(name)).collect();
        params.not_before = spec.not_before;
        params.not_after = spec.not_after;
        let leaf = params
            .signed_by(key, &self.issuer)
            .expect("signing a test leaf");
        let cert_pem = leaf.pem();
        Issued {
            chain_pem: format!("{cert_pem}{}", self.pem),
            cert_pem,
            key_pem: key.serialize_pem(),
            ca_pem: self.pem.clone(),
        }
    }
}

fn san(name: &str) -> SanType {
    match name.parse::<std::net::IpAddr>() {
        Ok(ip) => SanType::IpAddress(ip),
        Err(_) => SanType::DnsName(name.try_into().expect("a valid test DNS name")),
    }
}
