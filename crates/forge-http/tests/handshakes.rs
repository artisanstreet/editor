use super::*;

#[test]
fn one_peer_stalling_handshakes_leaves_room_for_every_other_peer() {
    let handshakes = Handshakes::new(4, 2);
    let attacker: IpAddr = "192.0.2.7".parse().unwrap();
    let editor: IpAddr = "::ffff:192.0.2.8".parse().unwrap();
    let stalled: Vec<Slot> = (0..2)
        .map(|_| handshakes.admit(attacker).unwrap())
        .collect();
    assert!(matches!(
        handshakes.admit(attacker),
        Err(Refused::Peer { in_flight: 2, .. })
    ));
    let rotated: IpAddr = "2001:db8::1".parse().unwrap();
    let same_network: IpAddr = "2001:db8::2".parse().unwrap();
    let _first = handshakes.admit(rotated).unwrap();
    let _second = handshakes.admit(same_network).unwrap();
    assert!(
        matches!(
            handshakes.admit("2001:db8::3".parse().unwrap()),
            Err(Refused::Peer { .. })
        ),
        "addresses rotated within one IPv6 network count as one peer"
    );
    assert!(matches!(
        handshakes.admit(editor),
        Err(Refused::Full { in_flight: 4 })
    ));
    drop(stalled);
    let _editor = handshakes
        .admit(editor)
        .expect("finished handshakes free their slots");
    let _again = handshakes.admit(attacker).unwrap();
}
