use super::*;

#[test]
fn claude_max_on_the_twenty_fold_tier_is_ultra() {
    assert_eq!(
        claude("max", Some("default_claude_max_20x")),
        Some("Claude Ultra")
    );
    assert_eq!(
        claude("max", Some("default_claude_max_5x")),
        Some("Claude Max")
    );
    assert_eq!(claude("max", None), Some("Claude Max"));
    assert_eq!(claude("pro", None), Some("Claude Pro"));
    assert_eq!(claude("mystery", None), None);
}
