use wg_2024::tests;

#[test]
fn fragment_forward() {
    tests::generic_fragment_forward::<dr_ones::Drone>();
}

#[test]
fn fragment_drop() {
    tests::generic_fragment_drop::<dr_ones::Drone>();
}

#[test]
fn chain_fragment_drop() {
    tests::generic_chain_fragment_drop::<dr_ones::Drone>();
}

#[test]
fn chain_fragment_ack() {
    tests::generic_chain_fragment_ack::<dr_ones::Drone>();
}
