mod config;
mod model;
mod options;
mod transport;

pub use config::MechanicsConfig;
pub use model::{
    EndpointBodyType, EndpointRetryPolicy, HttpEndpoint, QuerySpec, SlottedQueryMode, UrlParamSpec,
};
pub use transport::{
    EndpointHttpClient, EndpointHttpRequest, EndpointHttpRequestBody, EndpointHttpResponse,
    HttpMethod, ReqwestEndpointHttpClient,
};

pub(crate) use model::PreparedHttpEndpoint;
pub(crate) use options::{
    EndpointCallBody, EndpointCallOptions, EndpointResponse, EndpointResponseBody,
};
pub(crate) use transport::into_io_error;

#[cfg(test)]
pub(crate) use options::parse_endpoint_call_options;
