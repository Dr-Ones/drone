use common::NetworkUtils;
use crossbeam_channel::{select, Receiver, Sender};
use indexmap::IndexSet;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use wg_2024::controller::{DroneCommand, DroneEvent};
pub use wg_2024::drone::Drone;
use wg_2024::network::{NodeId, SourceRoutingHeader};
use wg_2024::packet::{Nack, NackType, NodeType, Packet, PacketType};

#[allow(non_camel_case_types)]
pub struct Dr_One {
    id: NodeId,
    sim_contr_send: Sender<DroneEvent>,
    sim_contr_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    pdr: f32,
    seen_flood_ids: IndexSet<u64>, //TODO: this should and can be a HashSet
    random_generator: StdRng, //TODO: remove from the drone
    should_exit:bool,
}

impl NetworkUtils for Dr_One {
    fn get_id(&self) -> NodeId {
        self.id
    }
    
    fn get_packet_senders(&self) -> &HashMap<NodeId, Sender<Packet>> {
        &self.packet_send
    }

    fn get_packet_receiver(&self) -> &Receiver<Packet> {
        &self.packet_recv
    }

    fn get_random_generator(&mut self) -> &mut StdRng {
        &mut self.random_generator
    }
}


impl Drone for Dr_One {
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
            pdr,
            packet_send,
            seen_flood_ids: IndexSet::new(),
            random_generator: StdRng::from_entropy(),
            should_exit:false,
        }
    }
    
    fn run(&mut self) {
        self.run_internal();
    }
}

impl Dr_One {
    fn run_internal(&mut self) {
        while !self.should_exit {
            select! {
                // handle receiving a packet from another drone
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        match packet.pack_type{
                            // flood request are particular because the recipient is not specified
                            PacketType::FloodRequest(ref _flood_req) => self.handle_flood_request(packet),
                            _ => self.handle_routed_packet(packet),
                        }
                    }
                },
                
                // handle receiving a message from the simulation controller
                recv(self.sim_contr_recv) -> command_res => {
                    if let Ok(command) = command_res {
                        
                        // each match branch may call a routine to handle it to make it more readable
                        match command {
                            DroneCommand::AddSender(node_id,sender) => self.add_channel(node_id,sender),
                            DroneCommand::SetPacketDropRate(new_pdr) => self.set_pdr(new_pdr),
                            DroneCommand::Crash => self.crash(),
                            DroneCommand::RemoveSender(node_id) => self.remove_channel(node_id),
                        }
                    }
                }
            }
        }
    }
    
    // add the node with identifier 'id' and crossbeam channel sender 'sender' to the list of neighbour nodes of self
    fn add_channel(&mut self, id: NodeId, sender: Sender<Packet>) {
        self.packet_send.insert(id, sender);
    }
    
    // remove the neighbour node of id 'id' from the list of neighbour nodes of self
    fn remove_channel(&mut self, id: NodeId) {
        self.packet_send.get(&id).expect(&format!(
            "Error ! The current node {} has no neighbour node {}.",
            self.id, id
        ));
        self.packet_send.remove(&id);
    }
    
    // Handle routed packet and check if its routing is correct. All the packets are routed except flood requests.
    // If the routing is correct then process it depending on its type else send back a nack.
    fn handle_routed_packet(&mut self, mut packet: Packet) {
        // eprintln!("[DRONE {}] I am handling packet {}.", self.id, packet.session_id.clone());
        
        // 1. Check if the drone is the expected recipient of the packet
        let index = packet.routing_header.hop_index;
        
        if self.id != packet.routing_header.hops[index] {
            // the drone is not the expected recipient. A nack of type UnexpectedRecipient needs to be sent back
            packet.routing_header.hop_index += 1;
            let packet = self.build_nack(packet, NackType::UnexpectedRecipient(self.id.clone()));
            self.forward_packet(packet);
            return;
        }
        
        // the drone is the expected recipient of the packet
        
        // 2. Increment hop_index by 1
        packet.routing_header.hop_index += 1;
        
        // 3. Determine if the drone is the final destination of the packet
        if packet.routing_header.hop_index == packet.routing_header.hops.len() {
            // the drone is the final destination of the packet. A nack needs to be sent back
            let packet = self.build_nack(packet, NackType::DestinationIsDrone);
            self.forward_packet(packet);
            return;
        }
        
        // the drone is not the final destination of the packet
        
        // 4. Identify the next hop using hops[hop_index], this node is called next_hop.
        // If next_hop is not a neighbour of self then a nack needs to be sent back.
        
        let next_hop_id = packet.routing_header.hops[packet.routing_header.hop_index];
        
        let is_not_a_neighbour: bool = matches!(self.packet_send.get(&next_hop_id), None);
        
        if is_not_a_neighbour {
            // next_hop is not a neighbour of self
            let error_in_routing_packet = self.build_nack(packet, NackType::ErrorInRouting(next_hop_id));
            self.forward_packet(error_in_routing_packet);
            return;
        }
        
        // next_hop is a neighbour of self
        
        // 5. Proceed based on the packet type
        match packet.pack_type {
            PacketType::Nack(ref _nack) => self.forward_packet(packet),
            PacketType::Ack(ref _ack) => self.forward_packet(packet),
            PacketType::FloodResponse(ref _flood_res) => {
                // eprintln!("[DRONE {}] forwarding flood response with path trace: {:?}", self.id, packet.routing_header.hops);
                self.forward_packet(packet)
            },
            PacketType::MsgFragment(ref _fragment) => {
                // a. Determine whether to drop the packet based on the drone's Packet Drop Rate (PDR).
                
                let pdr_scaled = (self.pdr * 100.0) as i32;
                let random_number = rand::thread_rng().gen_range(0..=100);
                
                let is_dropped: bool = random_number < pdr_scaled;
                
                if is_dropped {
                    eprintln!("[DRONE {}] : I drop a packet and send a Nack.",self.id);
                    // the packet is dropped. A nack needs to be sent back
                    let packet = self.build_nack(packet, NackType::Dropped);
                    self.forward_packet(packet);
                    return;
                }
                
                // the packet is not dropped
                
                // b. Send the packet to the neighbour
                
                self.forward_packet(packet);
            }
            _ => eprintln!("Received unhandled packet type: {:?}", packet.pack_type), //for debugging purpose
        }
    }
    
    // handle a received flood request depending on the neighbours of the drone and on the flood request
    fn handle_flood_request(&mut self, packet: Packet) {
        // Check if the flood request should be broadcast or turned into a flood response and sent back
        if let PacketType::FloodRequest(mut flood_request) = packet.pack_type.clone() {
            // Take who sent this floodRequest (test and logpurposes)
            let who_sent_me_this_flood_request = flood_request.path_trace.last().unwrap().0;
            
            // Add self to the path trace
            flood_request.path_trace.push((self.id, NodeType::Drone));
            
            // 1. Process some tests on the drone and its neighbours to know how to handle the flood request
            
            // a. Check if the drone has already received the flood request
            let flood_request_is_already_received: bool = self
            .seen_flood_ids
            .iter()
            .any(|id| *id == flood_request.flood_id);
            
            // b. Check if the drone has a neighbour, excluding the one from which it received the flood request
            
            // Check if the updated neighbours list is empty
            
            // If I have only one neighbour, I must have received this message from it and i don't have anybody else to forward it to
            let has_no_neighbour: bool = self.packet_send.len() == 1;
            
            // 2. Check if the flood request should be sent back as a flood response or broadcast as is
            if flood_request_is_already_received || has_no_neighbour {
                // A flood response should be created and sent
                
                // a. Create a build response based on the build request
                
                let flood_response_packet =
                self.build_flood_response(packet, flood_request.path_trace);
                
                // b. forward the flood response back
                // eprintln!(
                //     "[DRONE {}] Sending FloodResponse sess_id:{} whose path is: {:?}",
                //     self.id,
                //     flood_response_packet.session_id,
                //     flood_response_packet.routing_header.hops
                // );
                self.forward_packet(flood_response_packet);
            } else {
                // The packet should be broadcast
                // eprintln!("Drone id: {} -> flood_request with path_trace: {:?} broadcasted to peers: {:?}", self.id, flood_request.path_trace, self.packet_send.keys());
                self.seen_flood_ids.insert(flood_request.flood_id);
                
                // Create the new packet with the updated flood_request
                let updated_packet = Packet {
                    pack_type: PacketType::FloodRequest(flood_request),
                    routing_header: packet.routing_header,
                    session_id: packet.session_id,
                };
                
                // Broadcast the updated packet
                self.broadcast_packet(updated_packet, who_sent_me_this_flood_request);
            }
        } else {
            eprintln!("Error: the packet to be broadcast is not a flood request.");
        }
    }
    
    // Return a packet which pack_type attribute is nack of type NackType
    fn build_nack(&self, packet: Packet, nack_type: NackType) -> Packet {
        // 1. Keep in the nack the fragment index if the packet contains a fragment
        let frag_index: u64;
        
        if let PacketType::MsgFragment(fragment) = &packet.pack_type {
            frag_index = fragment.fragment_index;
        } else {
            frag_index = 0;
        }
        
        // 2. Build the Nack instance of the packet to return
        let nack: Nack = Nack {
            fragment_index: frag_index,
            nack_type,
        };
        
        // 3. Build the packet
        let packet_type = PacketType::Nack(nack);
        
        let mut packet: Packet = Packet {
            pack_type: packet_type,
            routing_header: packet.routing_header,
            session_id: packet.session_id,
        };
        
        // 4. Reverse the routing direction of the packet because nacks need to be sent back
        self.reverse_packet_routing_direction(&mut packet);
        
        // 5. Return the packet
        packet
    }
    
    // reverse the packet route in order it to be sent back.
    // In the end, the packet should go to the node (server or client) that initially routed the packet.
    fn reverse_packet_routing_direction(&self, packet: &mut Packet) {
        // a. create the route back using the path trace of the packet
        let mut hops_vec: Vec<NodeId> = packet.routing_header.hops.clone();
        
        // remove the nodes that are not supposed to receive the packet anymore (between self and the original final destination of the packet)
        hops_vec.drain(packet.routing_header.hop_index..=hops_vec.len() - 1);
        
        // reverse the order of the nodes to reach in comparison with the original routing header
        hops_vec.reverse();
        
        let route_back: SourceRoutingHeader = SourceRoutingHeader {
            //THE SOURCEROUTINGHEADER SAYS THAT THE HOP INDEX SHOULD BE INITIALIZED AS 1, BUT
            //KEEP IN MIND THAT IN THIS WAY THE NODE THAT RECEIVES THIS PACKET WILL SEE ITSELF IN THE PATH_TRACE[HOP_INDEX]
            hop_index: 1, // Start from the first hop
            hops: hops_vec,
        };
        
        // b. update the packet's routing header
        packet.routing_header = route_back;
    }
    
    // forward packet to a selected group of nodes in a flooding context
    fn broadcast_packet(&self, packet: Packet, who_i_received_the_packet_from: NodeId) {
        // Copy the list of the neighbours and remove the neighbour drone that sent the flood request
        let neighbours: HashMap<NodeId, Sender<Packet>> = self
        .packet_send
        .iter()
        .filter(|(&key, _)| key != who_i_received_the_packet_from)
        .map(|(k, v)| (*k, v.clone()))
        .collect();
        
        // iterate on the neighbours list
        for (&node_id, sender) in neighbours.iter() {
            // Send a clone packet
            if let Err(e) = sender.send(packet.clone()) {
                println!("Failed to send packet to NodeId {:?}: {:?}", node_id, e);
            }
        }
    }
    
    // set the packet drop rate of the drone as 'new_packet_drop_rate'
    fn set_pdr(&mut self, new_packet_drop_rate: f32) {
        self.pdr = new_packet_drop_rate;
    }
    
    // crash the drone
    fn crash(&mut self) {
        println!("[DRONE {}] Starting crash sequence", self.id);
        
        // Handle remaining packets
        println!("[DRONE {}] Processing remaining packets...", self.id);
        while let Ok(packet) = self.packet_recv.recv() {
            match packet.pack_type {
                PacketType::FloodRequest(_) => self.handle_flood_request(packet),
                _ => self.handle_routed_packet(packet),
            }
        }
        
        // Set exit flag
        self.should_exit = true;
    }
    
    // ------------------------------------------------------------------------------------------------
    // -------------------------------------- TEST FUNCTIONS ------------------------------------------
    // ------------------------------------------------------------------------------------------------
    
    // crash the drone
    fn crash_log(&mut self) {
        
        use std::fs::OpenOptions;
        use std::io::Write;
        // Define the log file path
        let log_path = "tests/crash_test/log.txt";
        
        // Open the log file in write mode
        let mut log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
        .expect("Failed to open or create log file");
        
        let log_msg = format!("[DRONE {}] Starting crash sequence", self.id);
        eprintln!("{}", log_msg);
        log_file.write_all(log_msg.as_bytes()).expect("Failed to write to log file");
        
        // Handle remaining packets
        let log_msg = format!("[DRONE {}] Processing remaining packets...", self.id);
        eprintln!("{}", log_msg);
        log_file.write_all(log_msg.as_bytes()).expect("Failed to write to log file");
        
        while let Ok(packet) = self.packet_recv.recv() {
            match packet.pack_type {
                PacketType::FloodRequest(_) => self.handle_flood_request(packet),
                _ => self.handle_routed_packet(packet),
            }
        }
        
        // Set exit flag
        self.should_exit = true;

        // Handle remaining packets
        let log_msg = format!("[DRONE {}] CRASHED.", self.id);
        eprintln!("{}", log_msg);
        log_file.write_all(log_msg.as_bytes()).expect("Failed to write to log file");
        
    }
    
    pub fn run_crash_test(&mut self) {
        while !self.should_exit {
            select! {
                // handle receiving a packet from another drone
                recv(self.packet_recv) -> packet_res => {
                    if let Ok(packet) = packet_res {
                        match packet.pack_type{
                            // flood request are particular because the recipient is not specified
                            PacketType::FloodRequest(ref _flood_req) => self.handle_flood_request(packet),
                            _ => self.handle_routed_packet(packet),
                        }
                    }
                },
                
                // handle receiving a message from the simulation controller
                recv(self.sim_contr_recv) -> command_res => {
                    if let Ok(command) = command_res {
                        
                        // each match branch may call a routine to handle it to make it more readable
                        match command {
                            DroneCommand::AddSender(node_id,sender) => self.add_channel(node_id,sender),
                            DroneCommand::SetPacketDropRate(new_pdr) => self.set_pdr(new_pdr),
                            DroneCommand::Crash => self.crash_log(),
                            DroneCommand::RemoveSender(node_id) => self.remove_channel(node_id),
                        }
                    }
                }
            }
        }
    }
    
    
}

