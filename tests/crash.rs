use crossbeam_channel::unbounded;
use std::collections::HashMap;
use std::thread;
use wg_2024::{
    controller::DroneCommand,
    drone::Drone,
    network::SourceRoutingHeader,
    packet::{Fragment, Nack, NackType, Packet, PacketType},
};

/// Creates a sample packet for testing purposes.
/// Using 1-10 for clients, 11-20 for drones and 21-30 for servers
fn create_sample_packet() -> Packet {
    Packet {
        pack_type: PacketType::MsgFragment(Fragment {
            fragment_index: 1,
            total_n_fragments: 1,
            length: 128,
            data: [1; 128],
        }),
        routing_header: SourceRoutingHeader {
            hop_index: 1,
            hops: vec![1, 11, 12, 21],
        },
        session_id: 1,
    }
}

/// Tests the crashing behavior of a drone.
fn generic_drone_crash<T: Drone + Send + 'static>() {
    // Client<1> channels
    let (c_send, c_recv) = unbounded();
    // Server<21> channels
    let (s_send, _s_recv) = unbounded();
    // Drone 11
    let (d_send, d_recv) = unbounded();
    // Drone 12
    let (d12_send, d12_recv) = unbounded();

    // SC - needed to send a RemoveSender command to 'd'
    let (d_command_send, d_command_recv) = unbounded();

    // SC - needed to send a crash command to 'd2'
    let (d2_command_send, d2_command_recv) = unbounded();

    // Drone 11
    let neighbours11 = HashMap::from([(12, d12_send.clone()), (1, c_send.clone())]);
    let mut drone = T::new(
        11,
        unbounded().0,
        d_command_recv,
        d_recv.clone(),
        neighbours11,
        0.0,
    );

    // Drone 12
    let neighbours12 = HashMap::from([(11, d_send.clone()), (21, s_send.clone())]);
    let mut drone2 = T::new(
        12,
        unbounded().0,
        d2_command_recv,
        d12_recv.clone(),
        neighbours12,
        0.0,
    );

    // Spawn drones in separate threads
    thread::spawn(move || {
        drone.run();
    });

    thread::spawn(move || {
        drone2.run();
    });

    // Send a RemoveSender to d before crashing d2
    let remove_sender_command = DroneCommand::RemoveSender(12);
    d_command_send.send(remove_sender_command).expect("Failed to send RemoveSender command");

    // Send a crash command to d2
    let crash_command = DroneCommand::Crash;
    d2_command_send.send(crash_command).expect("Failed to send Crash command");

    let msg = create_sample_packet();

    // "Client" sends packet to the drone
    d_send.send(msg.clone()).expect("Failed to send packet to the drone");

    // Client should receive a NACK since d2 has crashed and is unreachable
    assert_eq!(
        c_recv.recv().expect("Failed to receive packet"),
        Packet {
            pack_type: PacketType::Nack(Nack {
                nack_type: NackType::ErrorInRouting(12),
                fragment_index: 1,
            }),
            routing_header: SourceRoutingHeader {
                hop_index: 1,
                hops: vec![11, 1],
            },
            session_id: 1,
        }
    );
}

#[test]
fn test_drone_crash() {
    generic_drone_crash::<dr_ones::Drone>();
}
