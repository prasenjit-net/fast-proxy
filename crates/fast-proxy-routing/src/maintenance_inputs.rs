use fast_proxy_model::{MaintenanceSettings, MaintenanceTarget};

/// The maintenance scopes to compile into a snapshot. They are not part of
/// `Config` (a rollback never changes them), so the compile step takes them
/// beside it.
#[derive(Debug, Clone, Default)]
pub struct MaintenanceInputs {
    /// Every stored scope, expired ones included.
    pub scopes: Vec<(MaintenanceTarget, MaintenanceSettings)>,
    /// The time the snapshot is compiled at: a scope already over then is left
    /// out. A scope that ends later is still checked against the clock on every
    /// request.
    pub now_ms: i64,
}

impl MaintenanceInputs {
    /// No maintenance anywhere.
    pub fn none() -> Self {
        Self::default()
    }
}
