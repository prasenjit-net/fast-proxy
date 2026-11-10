//! Re-encodes a generated key in other PEM formats, so tests can cover them
//! without any private key committed to the repository.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Converts a PKCS#8 PEM holding a P-256 key (as `ring` and rcgen write it) to
/// a SEC1 `EC PRIVATE KEY` PEM. The PKCS#8 wrapper's inner OCTET STRING already
/// is the SEC1 structure, so this unwraps it.
pub fn pkcs8_pem_to_sec1_pem(pkcs8_pem: &str) -> String {
    let body: String = pkcs8_pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = STANDARD
        .decode(body)
        .expect("a PEM from rcgen is valid base64");
    let sec1 = inner_octet_string(&der).expect("a PKCS#8 P-256 key has an inner key");
    let encoded = STANDARD.encode(sec1);
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(64)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();
    format!(
        "-----BEGIN EC PRIVATE KEY-----\n{}\n-----END EC PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

/// Reads one DER element: `(tag, content, rest)`.
fn read(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        let (bytes, rest) = rest.split_at_checked(count)?;
        (
            bytes.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b)),
            rest,
        )
    };
    let (content, rest) = rest.split_at_checked(length)?;
    Some((tag, content, rest))
}

/// PKCS#8 is `SEQUENCE { version, SEQUENCE { algorithm }, OCTET STRING { key } }`.
fn inner_octet_string(pkcs8: &[u8]) -> Option<&[u8]> {
    let (_, sequence, _) = read(pkcs8)?;
    let (_, _version, rest) = read(sequence)?;
    let (_, _algorithm, rest) = read(rest)?;
    let (tag, key, _) = read(rest)?;
    (tag == 0x04).then_some(key)
}
