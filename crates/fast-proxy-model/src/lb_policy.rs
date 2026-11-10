use crate::FieldError;

const MAX_KEY_NAME_LEN: usize = 64;

/// How a group spreads requests across its eligible endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LbAlgorithm {
    #[default]
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    P2cEwma,
    ConsistentHash,
}

impl LbAlgorithm {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::WeightedRoundRobin => "weighted_round_robin",
            Self::LeastConnections => "least_connections",
            Self::P2cEwma => "p2c_ewma",
            Self::ConsistentHash => "consistent_hash",
        }
    }

    /// Parses the stored and API spelling.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        [
            Self::RoundRobin,
            Self::WeightedRoundRobin,
            Self::LeastConnections,
            Self::P2cEwma,
            Self::ConsistentHash,
        ]
        .into_iter()
        .find(|algorithm| algorithm.as_str() == raw)
        .ok_or_else(|| {
            FieldError::new(
                "lb.algorithm",
                "must be round_robin, weighted_round_robin, least_connections, p2c_ewma or consistent_hash",
            )
        })
    }
}

/// Where consistent hashing reads its key from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum HashKey {
    #[default]
    ClientIp,
    Header(String),
    Cookie(String),
}

impl HashKey {
    /// Parses `client_ip`, `header:<name>` or `cookie:<name>`.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let invalid = |message: &str| Err(FieldError::new("lb.hashKey", message));
        if raw == "client_ip" {
            return Ok(Self::ClientIp);
        }
        let Some((kind, name)) = raw.split_once(':') else {
            return invalid("must be client_ip, header:<name> or cookie:<name>");
        };
        if !is_token(name) {
            return invalid("the name must be 1 to 64 letters, digits or - _ .");
        }
        match kind {
            "header" => Ok(Self::Header(name.to_ascii_lowercase())),
            "cookie" => Ok(Self::Cookie(name.to_string())),
            _ => invalid("must be client_ip, header:<name> or cookie:<name>"),
        }
    }

    /// The stored and API spelling.
    pub fn to_spec(&self) -> String {
        match self {
            Self::ClientIp => "client_ip".to_string(),
            Self::Header(name) => format!("header:{name}"),
            Self::Cookie(name) => format!("cookie:{name}"),
        }
    }
}

fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_KEY_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A group's load-balancing choice.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LbPolicy {
    pub algorithm: LbAlgorithm,
    /// Only used by `ConsistentHash`; kept otherwise so switching back is lossless.
    pub hash_key: HashKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_algorithm_round_trips() {
        for algorithm in [
            LbAlgorithm::RoundRobin,
            LbAlgorithm::WeightedRoundRobin,
            LbAlgorithm::LeastConnections,
            LbAlgorithm::P2cEwma,
            LbAlgorithm::ConsistentHash,
        ] {
            assert_eq!(LbAlgorithm::parse(algorithm.as_str()).unwrap(), algorithm);
        }
        assert_eq!(
            LbAlgorithm::parse("random").unwrap_err().field,
            "lb.algorithm"
        );
    }

    #[test]
    fn default_is_round_robin_on_client_ip() {
        let policy = LbPolicy::default();
        assert_eq!(policy.algorithm, LbAlgorithm::RoundRobin);
        assert_eq!(policy.hash_key, HashKey::ClientIp);
    }

    #[test]
    fn hash_keys_parse_and_round_trip() {
        assert_eq!(HashKey::parse("client_ip").unwrap(), HashKey::ClientIp);
        assert_eq!(
            HashKey::parse("header:X-User-Id").unwrap(),
            HashKey::Header("x-user-id".into())
        );
        assert_eq!(
            HashKey::parse("cookie:Session").unwrap(),
            HashKey::Cookie("Session".into())
        );
        for key in [
            HashKey::ClientIp,
            HashKey::Header("x-a".into()),
            HashKey::Cookie("sid".into()),
        ] {
            assert_eq!(HashKey::parse(&key.to_spec()).unwrap(), key);
        }
    }

    #[test]
    fn bad_hash_keys_name_the_field() {
        let too_long = format!("header:{}", "a".repeat(65));
        for bad in ["", "ip", "header:", "header:a b", "path:x", &too_long] {
            assert_eq!(
                HashKey::parse(bad).unwrap_err().field,
                "lb.hashKey",
                "{bad}"
            );
        }
    }
}
