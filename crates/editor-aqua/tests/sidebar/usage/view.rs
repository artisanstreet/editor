use editor_aqua::sidebar::usage::view::token_count;
use editor_ui::locale::decimal;

#[test]
fn token_counts_carry_into_the_next_unit_when_rounding_reaches_it() {
    assert_eq!(token_count(999), "999 tokens");
    assert_eq!(token_count(1_250), format!("{}K tokens", decimal(1.3, 1)));
    assert_eq!(
        token_count(999_949),
        format!("{}K tokens", decimal(999.9, 1))
    );
    assert_eq!(token_count(999_960), format!("{}M tokens", decimal(1.0, 1)));
    assert_eq!(
        token_count(2_460_000),
        format!("{}M tokens", decimal(2.5, 1))
    );
}
