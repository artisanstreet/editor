#![expect(
    clippy::float_cmp,
    reason = "layout offsets and light levels are exact sums of fixed metrics, so equality is the contract"
)]

use super::*;

#[test]
fn switching_engines_again_mid_flight_continues_from_where_the_light_is() {
    let start = Instant::now();
    let mut light = EngineLight::default();
    light.measure(EngineId::Claude, 0.0, 32.0, start, false);
    assert_eq!(light.frame(start), Some((0.0, false)));
    light.measure(EngineId::Codex, 100.0, 32.0, start, false);
    let midway = start + Duration::from_millis(80);
    let (painted, moving) = light.frame(midway).unwrap();
    assert!(moving && painted > 0.0 && painted < 100.0);
    light.measure(EngineId::OpenCode, 200.0, 32.0, midway, false);
    assert_eq!(light.frame(midway).unwrap().0, painted);
    let later = light.frame(midway + Duration::from_millis(100)).unwrap().0;
    assert!(later > painted && later < 200.0);
    assert_eq!(
        light.frame(midway + Duration::from_secs(1)),
        Some((200.0, false))
    );
}

#[test]
fn reduced_motion_moves_the_light_without_a_flight() {
    let start = Instant::now();
    let mut light = EngineLight::default();
    light.measure(EngineId::Claude, 0.0, 32.0, start, true);
    light.measure(EngineId::Codex, 100.0, 32.0, start, true);
    assert!(!light.animating());
    assert_eq!(light.frame(start), Some((100.0, false)));
}
