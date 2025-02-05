//! Drone implementation for the dr_ones network simulator.
//! Provides the Dr_One type which implements the wg_2024::drone::Drone trait.
//!
//! # Logging Control
//! The drone and other network nodes produce log output that can be controlled globally:
//! ```rust
//! // Disable all node logging
//! dr_ones::disable_logging();
//!
//! // Re-enable logging
//! dr_ones::enable_logging();
//! ```

mod drone;
pub use drone::Drone;

// Re-export logging control functions
pub use network_node::{disable_logging, enable_logging, redirect_logs_to_file};
