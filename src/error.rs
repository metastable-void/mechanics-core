use std::{borrow::Cow, fmt::Display};

/// Error type used across script execution and pool operations.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MechanicsError {
    /// Script execution failed.
    Execution(Cow<'static, str>),
    /// Submission failed because the pool queue is full.
    QueueFull(Cow<'static, str>),
    /// Submission failed because enqueue timed out.
    QueueTimeout(Cow<'static, str>),
    /// Call failed because overall `run`/`run_try_enqueue` wait time elapsed.
    RunTimeout(Cow<'static, str>),
    /// Submission failed because the pool is closed.
    PoolClosed(Cow<'static, str>),
    /// Submission or result retrieval failed because no worker is available.
    WorkerUnavailable(Cow<'static, str>),
    /// Work item was canceled before execution.
    Canceled(Cow<'static, str>),
    /// Worker panicked while running a job.
    Panic(Cow<'static, str>),
    /// Pool setup or lifecycle management failed.
    RuntimePool(Cow<'static, str>),
}

impl MechanicsError {
    /// Builds an execution error.
    pub fn execution<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Execution(msg.into())
    }

    /// Builds a pool/runtime lifecycle error.
    pub fn runtime_pool<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::RuntimePool(msg.into())
    }

    /// Builds a queue-full error.
    pub fn queue_full<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::QueueFull(msg.into())
    }

    /// Builds a queue-timeout error.
    pub fn queue_timeout<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::QueueTimeout(msg.into())
    }

    /// Builds a run-timeout error.
    pub fn run_timeout<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::RunTimeout(msg.into())
    }

    /// Builds a pool-closed error.
    pub fn pool_closed<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::PoolClosed(msg.into())
    }

    /// Builds a worker-unavailable error.
    pub fn worker_unavailable<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::WorkerUnavailable(msg.into())
    }

    /// Builds a cancellation error.
    pub fn canceled<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Canceled(msg.into())
    }

    /// Builds a worker panic error.
    pub fn panic<M: Into<Cow<'static, str>>>(msg: M) -> Self {
        Self::Panic(msg.into())
    }

    /// Returns the raw error message.
    pub fn msg(&self) -> &str {
        match self {
            Self::Execution(msg) => msg.as_ref(),
            Self::QueueFull(msg) => msg.as_ref(),
            Self::QueueTimeout(msg) => msg.as_ref(),
            Self::RunTimeout(msg) => msg.as_ref(),
            Self::PoolClosed(msg) => msg.as_ref(),
            Self::WorkerUnavailable(msg) => msg.as_ref(),
            Self::Canceled(msg) => msg.as_ref(),
            Self::Panic(msg) => msg.as_ref(),
            Self::RuntimePool(msg) => msg.as_ref(),
        }
    }

    /// Returns the symbolic error kind name.
    pub fn kind(&self) -> &'static str {
        match &self {
            Self::Execution(_) => "MechanicsError::Execution",
            Self::QueueFull(_) => "MechanicsError::QueueFull",
            Self::QueueTimeout(_) => "MechanicsError::QueueTimeout",
            Self::RunTimeout(_) => "MechanicsError::RunTimeout",
            Self::PoolClosed(_) => "MechanicsError::PoolClosed",
            Self::WorkerUnavailable(_) => "MechanicsError::WorkerUnavailable",
            Self::Canceled(_) => "MechanicsError::Canceled",
            Self::Panic(_) => "MechanicsError::Panic",
            Self::RuntimePool(_) => "MechanicsError::RuntimePool",
        }
    }
}

impl Display for MechanicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.msg())
    }
}

impl std::error::Error for MechanicsError {}
