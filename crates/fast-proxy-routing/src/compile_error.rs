use thiserror::Error;

/// Why a configuration cannot be compiled into a snapshot. Each variant
/// is a cross-table rule that storage constraints cannot express.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompileError {
    #[error("subsystem \"{0}\" is defined more than once (same name or id)")]
    DuplicateSubsystem(String),
    #[error("host pattern \"{pattern}\" is used by both \"{first}\" and \"{second}\"")]
    DuplicateHost {
        pattern: String,
        first: String,
        second: String,
    },
    #[error("subsystem \"{subsystem}\" has more than one upstream named \"{name}\"")]
    DuplicateUpstreamName { subsystem: String, name: String },
    #[error("subsystem \"{subsystem}\" has more than one route with prefix \"{prefix}\"")]
    DuplicateRoutePrefix { subsystem: String, prefix: String },
    #[error(
        "route \"{prefix}\" in \"{subsystem}\" points at an upstream that does not exist in that subsystem"
    )]
    UnknownUpstream { subsystem: String, prefix: String },
    #[error(
        "upstream \"{upstream}\" in \"{subsystem}\" must have 1 to 64 endpoints, found {count}"
    )]
    EndpointCount {
        subsystem: String,
        upstream: String,
        count: usize,
    },
    #[error("upstream \"{upstream}\" in \"{subsystem}\" lists endpoint {addr} more than once")]
    DuplicateEndpoint {
        subsystem: String,
        upstream: String,
        addr: String,
    },
    #[error("upstream \"{upstream}\" in \"{subsystem}\" is invalid: {reason}")]
    InvalidUpstream {
        subsystem: String,
        upstream: String,
        reason: String,
    },
    #[error("subsystem \"{subsystem}\" has HTTPS enabled but no certificate")]
    HttpsWithoutCertificate { subsystem: String },
    #[error("subsystem \"{subsystem}\" references certificate {id}, which does not exist")]
    UnknownCertificate { subsystem: String, id: i64 },
    #[error(
        "upstream \"{upstream}\" in \"{subsystem}\" names CA bundle \"{name}\", which does not exist"
    )]
    UnknownCaBundle {
        subsystem: String,
        upstream: String,
        name: String,
    },
    #[error("maintenance is set for subsystem {id}, which does not exist")]
    UnknownMaintenanceSubsystem { id: i64 },
    #[error("maintenance is set for route {id}, which does not exist")]
    UnknownMaintenanceRoute { id: i64 },
    #[error("sampling is overridden for subsystem {id}, which does not exist")]
    UnknownSamplingSubsystem { id: i64 },
    #[error("sampling is overridden for route {id}, which does not exist")]
    UnknownSamplingRoute { id: i64 },
}
