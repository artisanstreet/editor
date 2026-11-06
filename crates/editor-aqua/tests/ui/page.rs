use editor_aqua::ui::page::preview_lines;

#[test]
fn preview_bounds_utf8_and_lines() {
    let long = "🎨".repeat(100_000);
    let shown = preview_lines(&long);
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0].len(), 4096);
    assert_eq!(shown[1], "…");
    let lines = "first\n".repeat(100_000);
    let shown = preview_lines(&lines);
    assert_eq!(shown.len(), 9);
    assert_eq!(shown.last().unwrap(), "…");
    assert_eq!(preview_lines(" \n\t"), ["(empty)"]);
    assert_eq!(preview_lines("a\nb"), ["a", "b"]);
}
