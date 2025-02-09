//! Drone implementation module.
//! Handles packet routing, flooding, and network management for drone nodes.

use crossbeam_channel::{select_biased, Receiver, Sender};
use network_node::{log_error, log_status, Command, NetworkNode};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::collections::{HashMap, HashSet};
use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    network::{NodeId, SourceRoutingHeader},
    packet::{Nack, NackType, NodeType, Packet, PacketType},
};

/// Implementation of a drone node in the network.
/// Responsible for routing packets and managing network connections.
pub struct Drone {
    id: NodeId,
    sim_contr_send: Sender<DroneEvent>,
    sim_contr_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    pdr: f32,
    seen_flood_ids: HashSet<String>,
    random_generator: StdRng,
    crashing_behavior: bool,
    should_exit: bool,
}

impl NetworkNode for Drone {
    /// Returns the unique identifier of the drone node.
    fn get_id(&self) -> NodeId {
        self.id
    }

    /// Indicates whether the drone is currently in crashing behavior mode.
    fn get_crashing_behavior(&self) -> bool {
        self.crashing_behavior
    }

    /// Provides mutable access to the set of flood packet identifiers that have been seen.
    fn get_seen_flood_ids(&mut self) -> &mut HashSet<String> {
        &mut self.seen_flood_ids
    }

    /// Returns mutable access to the mapping of node IDs to their respective packet sender channels.
    fn get_packet_send(&mut self) -> &mut HashMap<NodeId, Sender<Packet>> {
        &mut self.packet_send
    }

    /// Returns a reference to the packet receiver channel.
    fn get_packet_receiver(&self) -> &Receiver<Packet> {
        &self.packet_recv
    }

    /// Provides mutable access to the random number generator used by the drone.
    fn get_random_generator(&mut self) -> &mut StdRng {
        &mut self.random_generator
    }

    /// Returns a reference to the simulation controller event sender.
    fn get_sim_contr_send(&self) -> &Sender<DroneEvent> {
        &self.sim_contr_send
    }

    /// Processes a routed packet by verifying its routing header and forwarding it appropriately.
    ///
    /// Returns `true` if the packet was handled (e.g. responded to with a NACK) such that no further
    /// processing is needed, or `false` if the packet should continue being processed.
    fn handle_routed_packet(&mut self, packet: Packet) -> bool {
        if !self.verify_routing(&packet) {
            return false;
        }

        // Handle final destination
        if packet.routing_header.hop_index + 1 == packet.routing_header.hops.len() {
            if matches!(packet.pack_type, PacketType::MsgFragment(_)) {
                let nack = self.build_nack(packet, NackType::DestinationIsDrone);
                self.forward_packet(nack);
                return true;
            } else {
                if let Err(e) = self
                    .sim_contr_send
                    .send(DroneEvent::ControllerShortcut(packet.clone()))
                {
                    log_error!(self.id, "Failed to send ControllerShortcut event: {:?}", e);
                }
                self.forward_packet(packet);
                return false;
            }
        }

        let next_hop_id = packet.routing_header.hops[packet.routing_header.hop_index + 1];

        // Check if next hop is reachable
        if !self.packet_send.contains_key(&next_hop_id) {
            if matches!(packet.pack_type, PacketType::MsgFragment(_)) {
                // When building Nack for unreachable next hop,
                // we need to use the current packet state for route back
                let mut nack_packet = packet.clone();
                let nack = Nack {
                    fragment_index: match &packet.pack_type {
                        PacketType::MsgFragment(f) => f.fragment_index,
                        _ => 0,
                    },
                    nack_type: NackType::ErrorInRouting(next_hop_id),
                };
                nack_packet.pack_type = PacketType::Nack(nack);

                // Create return route from current position to source
                let mut hops: Vec<NodeId> =
                    packet.routing_header.hops[..=packet.routing_header.hop_index].to_vec();
                hops.reverse();
                nack_packet.routing_header = SourceRoutingHeader {
                    hop_index: 1, // Start at 1 since first hop is current node
                    hops: hops,
                };

                self.forward_packet(nack_packet);
            }
            return false;
        }

        match packet.pack_type {
            PacketType::MsgFragment(_) => {
                if self.crashing_behavior {
                    let nack = self.build_nack(packet, NackType::ErrorInRouting(self.get_id()));
                    self.forward_packet(nack);
                    return true;
                } else {
                    self.handle_message_fragment(packet);
                    return false;
                }
            }
            _ => {
                let mut forward_packet = packet.clone();
                forward_packet.routing_header.hop_index += 1;

                self.forward_packet(forward_packet);
                false
            }
        }
    }

    /// Handles a command received from the simulation controller by executing the corresponding action.
    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Drone(drone_command) => match drone_command {
                DroneCommand::AddSender(node_id, sender) => self.add_channel(node_id, sender),
                DroneCommand::SetPacketDropRate(new_pdr) => self.set_pdr(new_pdr),
                DroneCommand::Crash => self.crash(),
                DroneCommand::RemoveSender(node_id) => self.remove_channel(node_id),
            },
            _ => panic!("Drone {} received a wrong command type", self.get_id()),
        }
    }
}

impl wg_2024::drone::Drone for Drone {
    /// Creates a new instance of a Drone with the given parameters.
    fn new(
        id: NodeId,
        controller_send: Sender<DroneEvent>,
        controller_recv: Receiver<DroneCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
        pdr: f32,
    ) -> Self {
        Self {
            id,
            sim_contr_send: controller_send,
            sim_contr_recv: controller_recv,
            packet_recv,
            packet_send,
            pdr,
            seen_flood_ids: HashSet::new(),
            random_generator: StdRng::from_entropy(),
            should_exit: false,
            crashing_behavior: false,
        }
    }

    /// Executes the main event loop for the drone.
    ///
    /// The loop processes commands from the simulation controller and incoming packets until
    /// a termination condition is met.
    fn run(&mut self) {
        while !self.should_exit {
            select_biased! {
                recv(self.sim_contr_recv) -> command_res => {
                    if let Ok(drone_command) = command_res {
                        let command = Command::Drone(drone_command);
                        self.handle_command(command);
                    }
                },
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        self.should_exit = self.handle_packet(packet, NodeType::Drone);
                    }
                }
            }
        }
    }
}

impl Drone {
    /// Verifies the routing header of a packet to ensure it is addressed to the current node.
    ///
    /// If the packet is misrouted, a NACK is generated and forwarded.
    /// Returns `true` if the routing is valid, or `false` if a correction was needed.
    fn verify_routing(&mut self, packet: &Packet) -> bool {
        let index = packet.routing_header.hop_index;
        if self.id != packet.routing_header.hops[index] {
            let mut packet = packet.clone();
            packet.routing_header.hop_index += 1;
            let nack = self.build_nack(packet, NackType::UnexpectedRecipient(self.id));
            self.forward_packet(nack);
            return false;
        }
        true
    }

    /// Handles a message fragment packet.
    ///
    /// Depending on the packet drop decision (based on PDR), the packet may be dropped (with a NACK sent)
    /// or forwarded to the next hop by incrementing its routing header.
    fn handle_message_fragment(&mut self, packet: Packet) {
        if self.should_drop_packet() {
            // Send dropped event
            if let Err(e) = self
                .sim_contr_send
                .send(DroneEvent::PacketDropped(packet.clone()))
            {
                log_error!(self.id, "Failed to send PacketDropped event: {:?}", e);
            }

            // Build NACK for dropped packet
            let mut nack_packet = packet.clone();
            let nack = Nack {
                fragment_index: match &packet.pack_type {
                    PacketType::MsgFragment(f) => f.fragment_index,
                    _ => 0,
                },
                nack_type: NackType::Dropped,
            };
            nack_packet.pack_type = PacketType::Nack(nack);

            // Create return route from current position to source
            let mut hops: Vec<NodeId> =
                packet.routing_header.hops[..=packet.routing_header.hop_index].to_vec();
            hops.reverse();
            nack_packet.routing_header = SourceRoutingHeader {
                hop_index: 1, // Start at 1 since first hop is current node
                hops: hops,
            };

            self.forward_packet(nack_packet);
            return;
        }

        // Increment hop index for forwarding
        let mut forward_packet = packet.clone();
        forward_packet.routing_header.hop_index += 1;

        self.forward_packet(forward_packet);
    }

    /// Determines whether the packet should be dropped based on the current packet drop rate (PDR).
    ///
    /// Returns `true` if the packet is to be dropped, or `false` otherwise.
    fn should_drop_packet(&mut self) -> bool {
        let pdr_scaled = (self.pdr * 100.0) as i32;
        self.get_random_generator().gen_range(0..=100) < pdr_scaled
    }

    /// Sets the packet drop rate (PDR) for the drone.
    ///
    /// If the provided `new_pdr` is not within the range `[0.0, 1.0]`, an error is logged and the PDR remains unchanged.
    fn set_pdr(&mut self, new_pdr: f32) {
        if new_pdr < 0.0 || new_pdr > 1.0 {
            log_error!(self.id, "invalid PDR value: {}", new_pdr);
            return;
        }
        self.pdr = new_pdr;
    }

    /// Initiates the crash sequence for the drone.
    ///
    /// This method processes any remaining packets, updates the drone's state to indicate a crash,
    /// and logs the crash events.
    fn crash(&mut self) {
        log_status!(self.id, "Starting crash sequence");
        self.crashing_behavior = true;

        while let Ok(packet) = self.packet_recv.try_recv() {
            self.handle_packet(packet, NodeType::Drone);
        }

        self.should_exit = true;
        log_status!(self.id, "Crashed");
    }
}

#[cfg(test)]
mod tests {
    use wg_2024::drone::Drone as _;

    use super::*;

    /// Tests the creation of a Drone instance with the expected initial parameters.
    #[test]
    fn test_drone_creation() {
        let (controller_send, _) = crossbeam_channel::unbounded();
        let (_, controller_recv) = crossbeam_channel::unbounded();
        let (_, packet_recv) = crossbeam_channel::unbounded();
        let packet_send = HashMap::new();

        let drone = Drone::new(
            1,
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            0.0,
        );

        assert_eq!(drone.id, 1);
        assert_eq!(drone.pdr, 0.0);
        assert!(drone.packet_send.is_empty());
    }
}
