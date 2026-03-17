use crate::MechanicsConfig;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// One script execution request submitted to [`MechanicsPool`].
///
/// Jobs are expected to be self-contained for stateless runtime execution.
/// Do not assume any cross-job cache residency in workers.
/// When deserialized, `mod_source` must be non-empty.
#[derive(Debug, Clone)]
pub struct MechanicsJob {
    /// ECMAScript module source containing a `default` export callable.
    pub mod_source: Arc<str>,
    /// JSON argument passed to the script's default export.
    pub arg: Arc<Value>,
    /// Runtime configuration used for resolving `mechanics:endpoint` calls.
    pub config: Arc<MechanicsConfig>,
}

impl Serialize for MechanicsJob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("MechanicsJob", 3)?;
        state.serialize_field("mod_source", self.mod_source.as_ref())?;
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
        struct RawMechanicsJob {
            mod_source: String,
            arg: Value,
            config: MechanicsConfig,
        }

        let raw = RawMechanicsJob::deserialize(deserializer)?;
        if raw.mod_source.is_empty() {
            return Err(serde::de::Error::custom("mod_source must not be empty"));
        }

        Ok(Self {
            mod_source: Arc::<str>::from(raw.mod_source),
            arg: Arc::new(raw.arg),
            config: Arc::new(raw.config),
        })
    }
}

/// Per-job execution limits enforced by runtime workers.
#[derive(Debug, Clone, Copy)]
pub struct MechanicsExecutionLimits {
    /// Maximum wall-clock time allowed for one script execution.
    pub max_execution_time: Duration,
    /// Maximum loop iterations before the VM throws a runtime limit error.
    pub max_loop_iterations: u64,
    /// Maximum JS recursion depth before the VM throws a runtime limit error.
    pub max_recursion_depth: usize,
    /// Maximum VM stack size before the VM throws a runtime limit error.
    pub max_stack_size: usize,
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
        let job = MechanicsJob {
            mod_source: Arc::<str>::from("export default function main(arg) { return arg; }"),
            arg: Arc::new(json!({"hello": "world"})),
            config: Arc::new(config),
        };

        let encoded = serde_json::to_value(&job).expect("serialize job");
        let decoded: MechanicsJob = serde_json::from_value(encoded).expect("deserialize job");

        assert_eq!(decoded.mod_source.as_ref(), job.mod_source.as_ref());
        assert_eq!(decoded.arg.as_ref(), job.arg.as_ref());
        assert_eq!(decoded.config.endpoints.len(), 0);
    }

    #[test]
    fn mechanics_job_deserialize_rejects_empty_mod_source() {
        let err = serde_json::from_value::<MechanicsJob>(json!({
            "mod_source": "",
            "arg": null,
            "config": { "endpoints": {} }
        }))
        .expect_err("empty module source should be rejected");

        assert!(err.to_string().contains("mod_source must not be empty"));
    }
}
