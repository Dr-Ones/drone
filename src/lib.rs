//! Drone implementation for the dr_ones network simulator.
//! Provides the Dr_One type which implements the wg_2024::drone::Drone trait.

mod drone;
pub use drone::Drone;

// Re-export commonly used types from wg_2024
pub use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    network::NodeId,
    packet::Packet,
};
