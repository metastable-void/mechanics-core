//! `mechanics-core` executes JavaScript modules in worker-hosted Boa runtimes.
//!
//! # Stateless Execution Model
//! - The crate is designed for stateless, horizontally-scaled deployments.
//! - Each job should be self-contained: provide all input via [`MechanicsJob::arg`] and
//!   [`MechanicsJob::config`].
//! - Each job runs in an isolated JavaScript Realm, so `globalThis` mutations do not persist
//!   across jobs.
//! - Workers do not provide cross-job cache semantics or other shared mutable runtime state
//!   guarantees.
//! - Do not rely on in-process caching or worker affinity for correctness.
//! - If caching is required, keep it outside the process boundary (for example, external stores).
//!
mod error;
mod executor;
mod http;
mod job;
mod pool;
mod runtime;

pub use error::MechanicsError;
pub use http::{
    EndpointBodyType, EndpointHttpClient, EndpointHttpRequest, EndpointHttpRequestBody,
    EndpointHttpResponse, EndpointRetryPolicy, HttpEndpoint, HttpMethod, MechanicsConfig,
    QuerySpec, ReqwestEndpointHttpClient, SlottedQueryMode, UrlParamSpec,
};
pub use job::{MechanicsExecutionLimits, MechanicsJob};
pub use pool::{MechanicsPool, MechanicsPoolConfig, MechanicsPoolStats};
