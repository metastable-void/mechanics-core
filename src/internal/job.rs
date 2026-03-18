use crate::internal::{error::MechanicsError, http::MechanicsConfig};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// One script execution request submitted to [`crate::MechanicsPool`].
///
/// Jobs are expected to be self-contained for stateless runtime execution.
/// Do not assume any cross-job cache residency in workers.
/// When deserialized, `module_source` must be non-empty.
#[derive(Debug, Clone)]
pub struct MechanicsJob {
    /// ECMAScript module source containing a `default` export callable.
    pub(crate) module_source: Arc<str>,
    /// JSON argument passed to the script's default export.
    pub(crate) arg: Arc<Value>,
    /// Runtime configuration used for resolving `mechanics:endpoint` calls.
    pub(crate) config: Arc<MechanicsConfig>,
}

impl MechanicsJob {
    fn validate_module_source(module_source: &str) -> Result<(), MechanicsError> {
        if module_source.is_empty() {
            return Err(MechanicsError::runtime_pool(
                "module_source must not be empty",
            ));
        }
        Ok(())
    }

    /// Constructs a mechanics job with validated module source.
    pub fn new(
        module_source: impl Into<String>,
        arg: Value,
        config: MechanicsConfig,
    ) -> Result<Self, MechanicsError> {
        let module_source = module_source.into();
        Self::validate_module_source(&module_source)?;
        Ok(Self {
            module_source: Arc::<str>::from(module_source),
            arg: Arc::new(arg),
            config: Arc::new(config),
        })
    }

    /// Returns the ECMAScript module source.
    pub fn module_source(&self) -> &str {
        self.module_source.as_ref()
    }

    /// Returns the JSON argument passed to the module default export.
    pub fn arg(&self) -> &Value {
        self.arg.as_ref()
    }

    /// Returns the endpoint/runtime config used by this job.
    pub fn config(&self) -> &MechanicsConfig {
        self.config.as_ref()
    }

    pub(crate) fn into_parts(self) -> (Arc<str>, Arc<Value>, Arc<MechanicsConfig>) {
        (self.module_source, self.arg, self.config)
    }
}

impl Serialize for MechanicsJob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("MechanicsJob", 3)?;
        state.serialize_field("module_source", self.module_source.as_ref())?;
        state.serialize_field("arg", self.arg.as_ref())?;
        state.serialize_field("config", self.config.as_ref())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for MechanicsJob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawMechanicsJob {
            module_source: String,
            arg: Value,
            config: MechanicsConfig,
        }

        let raw = RawMechanicsJob::deserialize(deserializer)?;
        MechanicsJob::new(raw.module_source, raw.arg, raw.config).map_err(serde::de::Error::custom)
    }
}

/// Per-job execution limits enforced by runtime workers.
#[derive(Debug, Clone, Copy)]
pub struct MechanicsExecutionLimits {
    /// Maximum wall-clock time allowed for one script execution.
    pub(crate) max_execution_time: Duration,
    /// Maximum loop iterations before the VM throws a runtime limit error.
    pub(crate) max_loop_iterations: u64,
    /// Maximum JS recursion depth before the VM throws a runtime limit error.
    pub(crate) max_recursion_depth: usize,
    /// Maximum VM stack size before the VM throws a runtime limit error.
    pub(crate) max_stack_size: usize,
}

impl MechanicsExecutionLimits {
    /// Constructs validated execution limits.
    pub fn new(
        max_execution_time: Duration,
        max_loop_iterations: u64,
        max_recursion_depth: usize,
        max_stack_size: usize,
    ) -> Result<Self, MechanicsError> {
        let limits = Self {
            max_execution_time,
            max_loop_iterations,
            max_recursion_depth,
            max_stack_size,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub(crate) fn validate(&self) -> Result<(), MechanicsError> {
        if self.max_execution_time.is_zero() {
            return Err(MechanicsError::runtime_pool(
                "execution_limits.max_execution_time must be > 0",
            ));
        }
        if self.max_loop_iterations == 0 {
            return Err(MechanicsError::runtime_pool(
                "execution_limits.max_loop_iterations must be > 0",
            ));
        }
        if self.max_recursion_depth == 0 {
            return Err(MechanicsError::runtime_pool(
                "execution_limits.max_recursion_depth must be > 0",
            ));
        }
        if self.max_stack_size == 0 {
            return Err(MechanicsError::runtime_pool(
                "execution_limits.max_stack_size must be > 0",
            ));
        }
        Ok(())
    }

    /// Returns the max wall-clock execution time.
    pub fn max_execution_time(&self) -> Duration {
        self.max_execution_time
    }

    /// Returns the max loop-iteration limit.
    pub fn max_loop_iterations(&self) -> u64 {
        self.max_loop_iterations
    }

    /// Returns the max recursion-depth limit.
    pub fn max_recursion_depth(&self) -> usize {
        self.max_recursion_depth
    }

    /// Returns the max VM stack-size limit.
    pub fn max_stack_size(&self) -> usize {
        self.max_stack_size
    }
}

impl Default for MechanicsExecutionLimits {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_secs(10),
            max_loop_iterations: 1_000_000,
            max_recursion_depth: 512,
            max_stack_size: 10 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn mechanics_job_serde_roundtrip() {
        let config = MechanicsConfig::new(HashMap::new()).expect("create config");
        let job = MechanicsJob::new(
            "export default function main(arg) { return arg; }",
            json!({"hello": "world"}),
            config,
        )
        .expect("build job");

        let encoded = serde_json::to_value(&job).expect("serialize job");
        let decoded: MechanicsJob = serde_json::from_value(encoded).expect("deserialize job");

        assert_eq!(decoded.module_source(), job.module_source());
        assert_eq!(decoded.arg(), job.arg());
        assert_eq!(decoded.config().endpoints.len(), 0);
    }

    #[test]
    fn mechanics_job_deserialize_rejects_empty_module_source() {
        let err = serde_json::from_value::<MechanicsJob>(json!({
            "module_source": "",
            "arg": null,
            "config": { "endpoints": {} }
        }))
        .expect_err("empty module source should be rejected");

        assert!(err.to_string().contains("module_source must not be empty"));
    }

    #[test]
    fn mechanics_job_deserialize_rejects_unknown_fields() {
        let err = serde_json::from_value::<MechanicsJob>(json!({
            "module_source": "export default function main() { return null; }",
            "arg": null,
            "config": { "endpoints": {} },
            "unknown": true
        }))
        .expect_err("unknown fields should be rejected");

        assert!(err.to_string().contains("unknown field"));
    }
}
