use crate::FieldError;

const MAX_LEN: usize = 63;

/// A subsystem name: 1-63 characters of `[a-z0-9-]`, starting and ending
/// alphanumeric. It appears in UI URLs and metric labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubsystemName(String);

impl SubsystemName {
    /// Validates `raw` (no normalization: names must already be lowercase).
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let invalid = |message: &str| Err(FieldError::new("name", message));
        if raw.is_empty() || raw.len() > MAX_LEN {
            return invalid("must be 1 to 63 characters");
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return invalid("may contain only lowercase letters, digits, and '-'");
        }
        if raw.starts_with('-') || raw.ends_with('-') {
            return invalid("must start and end with a letter or digit");
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        for name in ["a", "shop", "my-shop", "shop2", "2shop", &"a".repeat(63)] {
            assert!(SubsystemName::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_invalid_names_with_a_name_field_error() {
        let too_long = "a".repeat(64);
        for name in [
            "",
            "-a",
            "a-",
            "Shop",
            "my_shop",
            "my shop",
            "a.b",
            too_long.as_str(),
        ] {
            let err = SubsystemName::parse(name).unwrap_err();
            assert_eq!(err.field, "name", "{name}");
        }
    }

    #[test]
    fn exposes_the_validated_text() {
        assert_eq!(SubsystemName::parse("shop").unwrap().as_str(), "shop");
    }
}
