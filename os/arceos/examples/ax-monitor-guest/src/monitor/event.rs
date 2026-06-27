use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CrashEvent {
    Panic,
    WatchdogTimeout,
    DoubleFault,
    Exception,
    Unknown,
}