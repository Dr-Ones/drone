//! Network utilities module.
//! Provides common functionality for network nodes (drones, clients, and servers).

use crossbeam_channel::Sender;
use rand::{rngs::StdRng, Rng};
use std::collections::HashMap;
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{NodeType, Packet, PacketType},
};

/// Common network functionality shared across different node types.
///
/// This trait provides basic network operations that all network nodes
/// (drones, clients, and servers) need to implement.
pub trait NetworkUtils {
    /// Returns the unique identifier of this network node.
    fn get_id(&self) -> NodeId;

    /// Returns a reference to the map of packet senders for connected nodes.
    fn get_packet_senders(&self) -> &HashMap<NodeId, Sender<Packet>>;

    /// Returns a mutable reference to the random number generator.
    fn get_random_generator(&mut self) -> &mut StdRng;

    /// Forwards a packet to the next hop in its routing path.
    ///
    /// # Arguments
    /// * `packet` - The packet to forward
    ///
    /// # Panics
    /// * If the next hop's sender channel is not found
    /// * If sending the packet fails
    fn forward_packet(&self, packet: Packet) {
        let next_hop_id = packet.routing_header.hops[packet.routing_header.hop_index];

        if let Some(sender) = self.get_packet_senders().get(&next_hop_id) {
            sender.send(packet).expect("Failed to forward the packet");
        } else {
            log_status(self.get_id(), &format!(
                "No channel found for next hop: {:?}",
                next_hop_id
            ));
        }
    }

    /// Builds a flood response packet based on a received flood request.
    ///
    /// # Arguments
    /// * `packet` - The original flood request packet
    /// * `updated_path_trace` - The updated path trace including this node
    ///
    /// # Returns
    /// * A new packet containing the flood response
    ///
    /// # Panics
    /// * If the input packet is not a flood request
    fn build_flood_response(
        &mut self,
        packet: Packet,
        updated_path_trace: Vec<(NodeId, NodeType)>,
    ) -> Packet {
        if let PacketType::FloodRequest(flood_request) = packet.pack_type {
            // Build the path trace for the response
            let mut route_back: Vec<NodeId> =
                updated_path_trace.iter().map(|tuple| tuple.0).collect();
            route_back.reverse();

            // Create the routing header for the response
            let new_routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: route_back,
            };

            // Create the flood response packet
            Packet {
                pack_type: PacketType::FloodResponse(wg_2024::packet::FloodResponse {
                    flood_id: flood_request.flood_id,
                    path_trace: updated_path_trace,
                }),
                routing_header: new_routing_header,
                session_id: self.get_random_generator().gen(),
            }
        } else {
            panic!("Error! Attempt to build flood response from non-flood request packet");
        }
    }
}

/// Helper function for consistent status logging
fn log_status(node_id: NodeId, message: &str) {
    println!("[NODE {}] {}", node_id, message);
}
