//! What the admin API may ask of the stored samples, validated before it is
//! turned into a database query. Trigger names become the bit mask the table
//! is filtered with.

use fast_proxy_model::{FieldError, Trigger, TriggerSet};

/// Most samples one page may hold.
pub const MAX_LIMIT: u32 = 500;
const MAX_NAME_LEN: usize = 256;

/// A filter over stored samples. Empty fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SampleQuery {
    /// Samples with any of these triggers (names in the API's spelling).
    pub triggers: Vec<String>,
    /// 1 to 5: the status's first digit.
    pub status_class: Option<u8>,
    pub subsystem: Option<String>,
    pub route: Option<String>,
    pub instance: Option<String>,
    pub client: Option<String>,
    pub min_ms: Option<u32>,
    pub since_ms: Option<i64>,
    /// A sample id: only older samples.
    pub before_id: Option<i64>,
    pub limit: Option<u32>,
}

impl SampleQuery {
    /// Checks every field; errors name the field in the API's spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        self.trigger_set()?;
        if let Some(class) = self.status_class {
            if !(1..=5).contains(&class) {
                return Err(FieldError::new("statusClass", "must be between 1 and 5"));
            }
        }
        if let Some(limit) = self.limit {
            if !(1..=MAX_LIMIT).contains(&limit) {
                return Err(FieldError::new(
                    "limit",
                    format!("must be between 1 and {MAX_LIMIT}"),
                ));
            }
        }
        for (field, value) in [
            ("subsystem", &self.subsystem),
            ("route", &self.route),
            ("instance", &self.instance),
            ("client", &self.client),
        ] {
            if value.as_ref().is_some_and(|v| v.len() > MAX_NAME_LEN) {
                return Err(FieldError::new(
                    field,
                    format!("must be at most {MAX_NAME_LEN} characters"),
                ));
            }
        }
        Ok(())
    }

    /// The triggers asked for as a set; an unknown name is an error.
    pub fn trigger_set(&self) -> Result<TriggerSet, FieldError> {
        self.triggers
            .iter()
            .map(|name| Trigger::parse("trigger", name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_is_valid_and_filters_nothing() {
        let query = SampleQuery::default();
        assert!(query.validate().is_ok());
        assert!(query.trigger_set().unwrap().is_empty());
    }

    #[test]
    fn trigger_names_become_the_bit_mask_for_every_trigger() {
        for trigger in Trigger::ALL {
            let query = SampleQuery {
                triggers: vec![trigger.as_str().into()],
                ..Default::default()
            };
            assert_eq!(query.trigger_set().unwrap().bits(), trigger.bit());
        }
        let both = SampleQuery {
            triggers: vec!["slow".into(), "retried".into()],
            ..Default::default()
        };
        assert_eq!(
            both.trigger_set().unwrap().bits(),
            Trigger::Slow.bit() | Trigger::Retried.bit()
        );
    }

    #[test]
    fn an_unknown_trigger_is_refused_by_name() {
        let query = SampleQuery {
            triggers: vec!["slow".into(), "server_error".into()],
            ..Default::default()
        };
        let err = query.validate().unwrap_err();
        assert_eq!(err.field, "trigger");
        assert!(err.message.contains("server_error"));
    }

    #[test]
    fn the_status_class_and_the_limit_are_bounded() {
        for class in [1u8, 5] {
            let query = SampleQuery {
                status_class: Some(class),
                ..Default::default()
            };
            assert!(query.validate().is_ok());
        }
        for class in [0u8, 6] {
            let query = SampleQuery {
                status_class: Some(class),
                ..Default::default()
            };
            assert_eq!(query.validate().unwrap_err().field, "statusClass");
        }
        for limit in [1u32, MAX_LIMIT] {
            let query = SampleQuery {
                limit: Some(limit),
                ..Default::default()
            };
            assert!(query.validate().is_ok());
        }
        for limit in [0u32, MAX_LIMIT + 1] {
            let query = SampleQuery {
                limit: Some(limit),
                ..Default::default()
            };
            assert_eq!(query.validate().unwrap_err().field, "limit");
        }
    }

    #[test]
    fn the_name_filters_are_length_bounded() {
        let long = "x".repeat(MAX_NAME_LEN + 1);
        for (field, query) in [
            (
                "subsystem",
                SampleQuery {
                    subsystem: Some(long.clone()),
                    ..Default::default()
                },
            ),
            (
                "route",
                SampleQuery {
                    route: Some(long.clone()),
                    ..Default::default()
                },
            ),
            (
                "instance",
                SampleQuery {
                    instance: Some(long.clone()),
                    ..Default::default()
                },
            ),
            (
                "client",
                SampleQuery {
                    client: Some(long.clone()),
                    ..Default::default()
                },
            ),
        ] {
            assert_eq!(query.validate().unwrap_err().field, field);
        }
    }
}
