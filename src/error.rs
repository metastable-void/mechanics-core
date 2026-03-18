use std::{borrow::Cow, fmt::Display};

/// Stable symbolic kind code for [`MechanicsError`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MechanicsErrorKind {
    Execution = 1,
    QueueFull = 2,
    QueueTimeout = 3,
    RunTimeout = 4,
    PoolClosed = 5,
    WorkerUnavailable = 6,
    Canceled = 7,
    Panic = 8,
    RuntimePool = 9,
}

impl MechanicsErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Execution => "MechanicsError::Execution",
            Self::QueueFull => "MechanicsError::QueueFull",
            Self::QueueTimeout => "MechanicsError::QueueTimeout",
            Self::RunTimeout => "MechanicsError::RunTimeout",
            Self::PoolClosed => "MechanicsError::PoolClosed",
            Self::WorkerUnavailable => "MechanicsError::WorkerUnavailable",
            Self::Canceled => "MechanicsError::Canceled",
            Self::Panic => "MechanicsError::Panic",
            Self::RuntimePool => "MechanicsError::RuntimePool",
        }
    }
}

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
    /// Call failed because overall `run`/`run_nonblocking_enqueue` wait time elapsed.
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
    pub fn worker_panic<M: Into<Cow<'static, str>>>(msg: M) -> Self {
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

    /// Returns the stable symbolic error kind.
    pub fn kind(&self) -> MechanicsErrorKind {
        match &self {
            Self::Execution(_) => MechanicsErrorKind::Execution,
            Self::QueueFull(_) => MechanicsErrorKind::QueueFull,
            Self::QueueTimeout(_) => MechanicsErrorKind::QueueTimeout,
            Self::RunTimeout(_) => MechanicsErrorKind::RunTimeout,
            Self::PoolClosed(_) => MechanicsErrorKind::PoolClosed,
            Self::WorkerUnavailable(_) => MechanicsErrorKind::WorkerUnavailable,
            Self::Canceled(_) => MechanicsErrorKind::Canceled,
            Self::Panic(_) => MechanicsErrorKind::Panic,
            Self::RuntimePool(_) => MechanicsErrorKind::RuntimePool,
        }
    }
}

impl Display for MechanicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind().as_str(), self.msg())
    }
}

impl std::error::Error for MechanicsError {}
