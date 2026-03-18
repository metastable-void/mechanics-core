mod api;
mod config;
mod drop_impl;
mod restart_guard;
mod shared;
mod worker;

pub use api::{MechanicsPool, MechanicsPoolStats};
pub use config::MechanicsPoolConfig;

#[cfg(test)]
pub(crate) use restart_guard::RestartGuard;
#[cfg(test)]
pub(crate) use shared::MechanicsPoolShared;
#[cfg(test)]
pub(crate) use worker::{PoolJob, PoolMessage, WorkerExit, WorkerHandle};

#[cfg(test)]
mod tests;
