mod config;
mod endpoint;
mod headers;
mod options;
mod query;
mod retry;
mod template;
mod transport;

pub use config::MechanicsConfig;
pub use endpoint::{EndpointBodyType, HttpEndpoint, QuerySpec, SlottedQueryMode, UrlParamSpec};
pub use retry::EndpointRetryPolicy;
pub use transport::{
    EndpointHttpClient, EndpointHttpHeaders, EndpointHttpRequest, EndpointHttpRequestBody,
    EndpointHttpResponse, HttpMethod, ReqwestEndpointHttpClient,
};

pub(crate) use endpoint::PreparedHttpEndpoint;
pub(crate) use options::{
    EndpointCallBody, EndpointCallOptions, EndpointResponse, EndpointResponseBody,
};
pub(crate) use transport::into_io_error;

#[cfg(test)]
pub(crate) use options::parse_endpoint_call_options;
