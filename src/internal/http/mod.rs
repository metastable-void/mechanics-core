mod endpoint;
mod headers;
mod options;
mod transport;
mod wrappers;

pub use mechanics_config::{EndpointBodyType, HttpEndpoint, MechanicsConfig, PreparedHttpEndpoint};
#[cfg(test)]
pub use mechanics_config::{
    EndpointRetryPolicy, HttpMethod, QuerySpec, SlottedQueryMode, UrlParamSpec,
};
pub use transport::{
    DefaultEndpointHttpClient, EndpointHttpClient, EndpointHttpHeaders, EndpointHttpRequest,
    EndpointHttpRequestBody, EndpointHttpResponse,
};

pub(crate) use endpoint::execute_endpoint;
pub(crate) use options::{
    EndpointCallBody, EndpointCallOptions, EndpointResponse, EndpointResponseBody,
};
pub(crate) use transport::into_io_error;
pub(crate) use wrappers::BoaMechanicsConfig;

#[cfg(test)]
pub(crate) use options::parse_endpoint_call_options;
