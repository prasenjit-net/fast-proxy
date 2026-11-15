use fast_proxy_model::AcmeChallenge;

/// The pending ACME HTTP-01 challenges, as stored. Like maintenance they are
/// not `Config` (a rollback never touches them), so compile takes them beside it.
#[derive(Debug, Clone, Default)]
pub struct AcmeInputs {
    /// Every stored challenge, expired ones included.
    pub challenges: Vec<AcmeChallenge>,
    /// The time the snapshot is compiled at: a challenge expired by then is left out.
    pub now_ms: i64,
}

impl AcmeInputs {
    /// No challenge pending.
    pub fn none() -> Self {
        Self::default()
    }
}
