use super::*;

#[test]
fn curve_sampling_is_bounded_and_keeps_both_endpoints() {
    let model: Concentration = serde_json::from_value(serde_json::json!({"values": vec![1; 1000]}))
        .expect("a valid concentration chart");
    let samples = sample(&model.curve().expect("a ranked curve"), 400);
    assert_eq!(samples.len(), 400);
    assert_eq!(samples.first(), Some(&(0.0, 0.0)));
    assert_eq!(samples.last(), Some(&(1.0, 1.0)));
}
