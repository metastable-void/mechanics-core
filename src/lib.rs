//! `mechanics-core` executes JavaScript modules in worker-hosted Boa runtimes.
//!
//! # Stateless Execution Model
//! - The crate is designed for stateless, horizontally-scaled deployments.
//! - Each job should be self-contained: provide all input via [`crate::job::MechanicsJob::arg`]
//!   and [`crate::job::MechanicsJob::config`].
//! - Each job runs in an isolated JavaScript Realm, so `globalThis` mutations do not persist
//!   across jobs.
//! - Workers do not provide cross-job cache semantics or other shared mutable runtime state
//!   guarantees.
//! - Do not rely on in-process caching or worker affinity for correctness.
//! - If caching is required, keep it outside the process boundary (for example, external stores).
//!

pub(crate) mod internal;

/// Job-related functionalities.
pub mod job {
    pub use crate::internal::http::MechanicsConfig;

    pub use crate::internal::job::{MechanicsExecutionLimits, MechanicsJob};
}

pub use internal::error::{MechanicsError, MechanicsErrorKind};

pub use internal::pool::{MechanicsPool, MechanicsPoolConfig, MechanicsPoolStats};

/// Endpoint related exports.
pub mod endpoint {
    pub use crate::internal::http::{
        EndpointBodyType, EndpointRetryPolicy, HttpEndpoint, HttpMethod, QuerySpec,
        SlottedQueryMode, UrlParamSpec,
    };

    /// Pluggable HTTP client module.
    pub mod http_client {
        pub use crate::internal::http::{
            EndpointHttpClient, EndpointHttpHeaders, EndpointHttpRequest, EndpointHttpRequestBody,
            EndpointHttpResponse, ReqwestEndpointHttpClient,
        };
    }
}
