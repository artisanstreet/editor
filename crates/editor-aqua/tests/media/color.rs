use editor_aqua::ui::media::color::{Notation, color_copy_entries, color_ranges, parse};

#[test]
fn prose_and_code_find_colors_without_interpreting_issue_numbers() {
    let text = "Emerald #10B981, rgb(255 0 0 / 50%) and #502; code #333";
    let code = text.find("#333").unwrap();
    let values: Vec<_> = color_ranges(text, &[code..code + 4])
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(values, ["#10B981", "rgb(255 0 0 / 50%)", "#333"]);
    assert!(color_ranges("foorgb(1 2 3), &#123; and page#abcdef0", &[]).is_empty());
}

#[test]
fn color_notations_preserve_transparency_and_convert_wide_gamut() {
    let color = parse("rgb(255 0 0 / 50%)").unwrap();
    assert_eq!(color.format(Notation::Hex), "#ff000080");
    assert_eq!(color.format(Notation::Rgb), "rgb(255 0 0 / 0.5)");
    assert!(parse("color(display-p3 1 0 0)").unwrap().r > 1.0);
    let entries = color_copy_entries(" #FF0000 ");
    assert_eq!(entries[0].text, "#FF0000");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>(),
        [
            "Copy Value",
            "Copy as HEX",
            "Copy as RGB",
            "Copy as HSL",
            "Copy as OKLCH",
            "Copy as OKLab",
            "Copy as Display P3"
        ]
    );
}
