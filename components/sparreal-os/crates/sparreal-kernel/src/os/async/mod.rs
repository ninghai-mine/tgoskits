//! Sparreal OS Async Runtime
//!
//! Single CPU async executor based on embassy design, providing tokio-like API experience.
//!
//! # Features
//!
//! - Single CPU async task execution
//! - Wake task priority scheduling
//! - Timeout task priority promotion (default 1 second)
//! - Interrupt safe (using IrqSpinlock)
//! - Dynamic memory allocation (alloc)
//!
//! # Usage Examples
//!
//! ```rust
//! use sparreal_kernel::os::async::{spawn, block_on, tick};
//!
//! // Spawn async task
//! let handle = spawn(async {
//!     // Async task code
//!     println!("Hello from async task!");
//! });
//!
//! // Block until task completes
//! block_on(async {
//!     // Your async code
//! });
//!
//! // Manual scheduling (in event loop)
//! loop {
//!     tick();  // Process one task scheduling
//!     // Other main loop logic...
//! }
//! ```

pub mod executor;
pub mod task;

// #[cfg(test)]
// mod tests;

// Re-export public interfaces
pub use executor::{SingleCpuExecutor, block_on, has_pending_tasks, spawn, task_count, tick};

pub use task::{TaskHandle, TaskId, TaskState};
