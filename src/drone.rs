//! Drone implementation module.
//! Handles packet routing, flooding, and network management for drone nodes.

use crossbeam_channel::{select_biased, Receiver, Sender};
use network_node::{Command, NetworkNode};
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
    seen_flood_ids: HashSet<u64>,
    random_generator: StdRng,
    crashing_behavior: bool,
    should_exit: bool,
}

impl NetworkNode for Drone {
    fn get_id(&self) -> NodeId {
        self.id
    }

    fn get_crashing_behavior(&self) -> bool {
        self.crashing_behavior
    }

    fn get_seen_flood_ids(&mut self) -> &mut HashSet<u64> {
        &mut self.seen_flood_ids
    }

    fn get_packet_send(&mut self) -> &mut HashMap<NodeId, Sender<Packet>> {
        &mut self.packet_send
    }

    fn get_packet_receiver(&self) -> &Receiver<Packet> {
        &self.packet_recv
    }

    fn get_random_generator(&mut self) -> &mut StdRng {
        &mut self.random_generator
    }

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
                    log_status(
                        self.id,
                        &format!("Failed to send ControllerShortcut event: {:?}", e),
                    );
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
                // TODO: is this meant to work this way????
                //  shouldn't the hop index be incremented in the forward_packet function??
                //  before implementig this directly in the function be sure that there are no cases in which this is not the desired behaviour
                //  if this gets implemented in the function, be sure to delete every place where the hop index is incremented before calling the function
                let mut forward_packet = packet.clone();
                forward_packet.routing_header.hop_index += 1;

                // Send PacketSent event for non-fragment packets too
                if let Err(e) = self
                    .sim_contr_send
                    .send(DroneEvent::PacketSent(forward_packet.clone()))
                {
                    log_status(
                        self.id,
                        &format!("Failed to send PacketSent event: {:?}", e),
                    );
                }
                self.forward_packet(forward_packet);
                false
            }
        }
    }

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

    /// Main event loop for the drone.
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

    // Handling a message fragment for a drone is very different from the behaviour that a client or a server would have
    fn handle_message_fragment(&mut self, packet: Packet) {
        if self.should_drop_packet() {
            // Send dropped event
            if let Err(e) = self
                .sim_contr_send
                .send(DroneEvent::PacketDropped(packet.clone()))
            {
                log_status(
                    self.id,
                    &format!("Failed to send PacketDropped event: {:?}", e),
                );
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

        if let Err(e) = self
            .sim_contr_send
            .send(DroneEvent::PacketSent(forward_packet.clone()))
        {
            log_status(
                self.id,
                &format!("Failed to send PacketSent event: {:?}", e),
            );
        }
        self.forward_packet(forward_packet);
    }

    fn should_drop_packet(&mut self) -> bool {
        let pdr_scaled = (self.pdr * 100.0) as i32;
        self.get_random_generator().gen_range(0..=100) < pdr_scaled
    }

    fn set_pdr(&mut self, new_pdr: f32) {
        self.pdr = new_pdr;
    }

    fn crash(&mut self) {
        log_status(self.id, "Starting crash sequence");

        while let Ok(packet) = self.packet_recv.try_recv() {
            self.handle_packet(packet, NodeType::Drone);
        }

        self.crashing_behavior = true;
        log_status(self.id, "Crashed");
    }
}

#[cfg(test)]
mod tests {
    use wg_2024::drone::Drone as _;

    use super::*;

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
