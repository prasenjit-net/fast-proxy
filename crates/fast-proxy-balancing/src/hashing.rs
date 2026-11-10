use std::net::IpAddr;

/// SplitMix64 finalizer: spreads bits so similar inputs hash far apart.
pub(crate) fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Hashes request key bytes (a header or cookie value) for consistent hashing.
/// Stable across runs and platforms.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    mix(hash)
}

/// Hashes a client address for consistent hashing, without allocating.
pub fn hash_ip(ip: IpAddr) -> u64 {
    match ip {
        IpAddr::V4(v4) => mix(u64::from(u32::from(v4))),
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            mix((bits >> 64) as u64 ^ mix(bits as u64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_distinguish_inputs() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
        let a = hash_ip("10.0.0.1".parse().unwrap());
        assert_eq!(a, hash_ip("10.0.0.1".parse().unwrap()));
        assert_ne!(a, hash_ip("10.0.0.2".parse().unwrap()));
        assert_ne!(
            hash_ip("::1".parse().unwrap()),
            hash_ip("::2".parse().unwrap())
        );
    }
}
