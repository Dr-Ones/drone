//! Drone implementation module.
//! Handles packet routing, flooding, and network management for drone nodes.

use common::{log_status, NetworkUtils};
use crossbeam_channel::{select, Receiver, Sender};
use indexmap::IndexSet;
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
    should_exit: bool,
}

impl NetworkUtils for Drone {
    fn get_id(&self) -> NodeId {
        self.id
    }

    fn get_packet_send(&self) -> &HashMap<NodeId, Sender<Packet>> {
        &self.packet_send
    }

    fn get_packet_receiver(&self) -> &Receiver<Packet> {
        &self.packet_recv
    }

    fn get_random_generator(&mut self) -> &mut StdRng {
        &mut self.random_generator
    }

    fn get_seen_flood_ids(&mut self) -> &mut HashSet<u64> {
        &mut self.seen_flood_ids
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
        }
    }

    /// Main event loop for the drone.
    fn run(&mut self) {
        while !self.should_exit {
            select! {
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        self.handle_packet(packet);
                    }
                },

                recv(self.sim_contr_recv) -> command_res => {
                    if let Ok(command) = command_res {
                        self.handle_command(command);
                    }
                }
            }
        }
    }
}

impl Drone {
    // TODO: This implementation should be in the client and server node as well
    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::FloodRequest(_) => self.handle_flood_request(packet, NodeType::Drone),
            _ => self.handle_routed_packet(packet),
        }
    }

    fn handle_command(&mut self, command: DroneCommand) {
        match command {
            DroneCommand::AddSender(node_id, sender) => self.add_channel(node_id, sender),
            DroneCommand::SetPacketDropRate(new_pdr) => self.set_pdr(new_pdr),
            DroneCommand::Crash => self.crash(),
            DroneCommand::RemoveSender(node_id) => self.remove_channel(node_id),
        }
    }

    fn should_respond_to_flood(&self, flood_request: &wg_2024::packet::FloodRequest) -> bool {
        self.seen_flood_ids.contains(&flood_request.flood_id) || self.packet_send.len() == 1
        // Only one neighbor means we can't forward
    }

    fn handle_routed_packet(&mut self, packet: Packet) {
        if !self.verify_routing(&packet) {
            return;
        }

        // Handle final destination
        if packet.routing_header.hop_index + 1 == packet.routing_header.hops.len() {
            let nack = self.build_nack(packet, NackType::DestinationIsDrone);
            self.forward_packet(nack);
            return;
        }

        let next_hop_id = packet.routing_header.hops[packet.routing_header.hop_index + 1];

        // Check if next hop is reachable
        if !self.packet_send.contains_key(&next_hop_id) {
            let nack = self.build_nack(packet, NackType::ErrorInRouting(next_hop_id));
            self.forward_packet(nack);
            return;
        }

        match packet.pack_type {
            PacketType::MsgFragment(_) => self.handle_message_fragment(packet),
            _ => {
                let mut forward_packet = packet.clone();
                forward_packet.routing_header.hop_index += 1;
                self.forward_packet(forward_packet)
            }
        }
    }

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

    fn handle_message_fragment(&mut self, packet: Packet) {
        if self.should_drop_packet() {
            // Send event before modifying packet
            if let Err(e) = self
                .sim_contr_send
                .send(DroneEvent::PacketDropped(packet.clone()))
            {
                log_status(
                    self.id,
                    &format!("Failed to send PacketDropped event: {:?}", e),
                );
            }
            let nack = self.build_nack(packet, NackType::Dropped);
            self.forward_packet(nack);
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

    fn add_channel(&mut self, id: NodeId, sender: Sender<Packet>) {
        self.packet_send.insert(id, sender);
    }

    fn remove_channel(&mut self, id: NodeId) {
        if !self.packet_send.contains_key(&id) {
            log_status(
                self.id,
                &format!(
                    "Error! The current node {} has no neighbour node {}.",
                    self.id, id
                ),
            );
            return;
        }
        self.packet_send.remove(&id);
    }

    fn set_pdr(&mut self, new_pdr: f32) {
        self.pdr = new_pdr;
    }

    fn crash(&mut self) {
        log_status(self.id, "Starting crash sequence");

        while let Ok(packet) = self.packet_recv.try_recv() {
            self.handle_packet(packet);
        }

        self.should_exit = true;
        log_status(self.id, "Crashed");
    }


    fn build_nack(&self, packet: Packet, nack_type: NackType) -> Packet {
        let fragment_index = match &packet.pack_type {
            PacketType::MsgFragment(fragment) => fragment.fragment_index,
            _ => 0,
        };

        let nack = Nack {
            fragment_index,
            nack_type,
        };

        let mut response = Packet {
            pack_type: PacketType::Nack(nack),
            routing_header: packet.routing_header,
            session_id: packet.session_id,
        };

        self.reverse_packet_routing_direction(&mut response);
        response
    }

    // TODO: Should be the same in client and server node (handled in handle_routed_packet)
    fn reverse_packet_routing_direction(&self, packet: &mut Packet) {
        let mut hops = packet.routing_header.hops[..=packet.routing_header.hop_index].to_vec();
        hops.reverse();

        packet.routing_header = SourceRoutingHeader { hop_index: 1, hops };
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

