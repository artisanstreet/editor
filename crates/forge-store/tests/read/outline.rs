use super::outline_label;

#[test]
fn labels_bound_giant_unicode_messages_and_use_first_nonempty_line() {
    assert_eq!(outline_label("\n  \n  hello  \nsecond"), "hello");
    let label = outline_label(&"界".repeat(10_000));
    assert_eq!(label.chars().count(), 161);
    assert!(label.ends_with('…'));
    assert_eq!(outline_label(""), "Image");
}
