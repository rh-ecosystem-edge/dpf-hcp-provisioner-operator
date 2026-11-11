//! Port of internal/controller/finalizer
//!
//! * [`CleanupHandler`] — `finalizer.CleanupHandler` (handler.go)
//! * [`Manager`] — `finalizer.Manager` (manager.go)

mod handler;
mod manager;

pub use handler::CleanupHandler;
pub use manager::Manager;

#[cfg(test)]
mod manager_test;
