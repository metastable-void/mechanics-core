use boa_engine::{JsData, Trace};
use boa_gc::Finalize;
use mechanics_config::MechanicsConfig;

#[derive(Trace, Finalize, JsData, Clone, Debug)]
pub(crate) struct BoaMechanicsConfig(#[unsafe_ignore_trace] MechanicsConfig);

impl From<MechanicsConfig> for BoaMechanicsConfig {
    fn from(value: MechanicsConfig) -> Self {
        Self(value)
    }
}

impl From<BoaMechanicsConfig> for MechanicsConfig {
    fn from(value: BoaMechanicsConfig) -> Self {
        value.0.clone()
    }
}

impl BoaMechanicsConfig {
    pub(crate) fn as_inner(&self) -> &MechanicsConfig {
        &self.0
    }
}
