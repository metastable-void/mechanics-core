use super::HttpEndpoint;
use crate::error::MechanicsError;
use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Serializable runtime data injected into the JS context.
///
/// This is intended to be supplied per job so workers remain stateless and horizontally scalable.
#[derive(JsData, Trace, Finalize, Serialize, Clone, Debug)]
pub struct MechanicsConfig {
    pub(crate) endpoints: HashMap<String, HttpEndpoint>,
}

impl MechanicsConfig {
    /// Builds runtime state from endpoint definitions.
    ///
    /// Provide the complete endpoint map needed by a job; workers do not maintain shared endpoint
    /// cache state across jobs.
    pub fn new(endpoints: HashMap<String, HttpEndpoint>) -> Result<Self, MechanicsError> {
        for (name, endpoint) in &endpoints {
            endpoint.validate_config().map_err(|e| {
                MechanicsError::runtime_pool(format!("invalid endpoint `{name}` config: {e}"))
            })?;
        }
        Ok(Self { endpoints })
    }

    /// Validates all configured endpoints.
    ///
    /// This method does not cache across jobs; it only checks consistency for the supplied
    /// configuration object.
    pub fn validate(&self) -> Result<(), MechanicsError> {
        for (name, endpoint) in &self.endpoints {
            endpoint.validate_config().map_err(|e| {
                MechanicsError::runtime_pool(format!("invalid endpoint `{name}` config: {e}"))
            })?;
        }
        Ok(())
    }

    /// Returns a new config with one endpoint inserted or replaced after validation.
    pub fn with_endpoint<S: Into<String>>(
        mut self,
        name: S,
        endpoint: HttpEndpoint,
    ) -> Result<Self, MechanicsError> {
        let name = name.into();
        endpoint.validate_config().map_err(|e| {
            MechanicsError::runtime_pool(format!("invalid endpoint `{name}` config: {e}"))
        })?;
        self.endpoints.insert(name, endpoint);
        Ok(self)
    }

    /// Returns a new config with all endpoint overrides validated and applied.
    ///
    /// Existing endpoints with matching names are replaced; other endpoints are retained.
    pub fn with_endpoint_overrides(
        mut self,
        overrides: HashMap<String, HttpEndpoint>,
    ) -> Result<Self, MechanicsError> {
        for (name, endpoint) in overrides {
            endpoint.validate_config().map_err(|e| {
                MechanicsError::runtime_pool(format!("invalid endpoint `{name}` config: {e}"))
            })?;
            self.endpoints.insert(name, endpoint);
        }
        Ok(self)
    }

    /// Returns a new config with one endpoint removed, if present.
    pub fn without_endpoint(mut self, name: &str) -> Self {
        self.endpoints.remove(name);
        self
    }
}

impl<'de> Deserialize<'de> for MechanicsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawMechanicsConfig {
            endpoints: HashMap<String, HttpEndpoint>,
        }

        let raw = RawMechanicsConfig::deserialize(deserializer)?;
        MechanicsConfig::new(raw.endpoints).map_err(serde::de::Error::custom)
    }
}
