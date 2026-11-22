use super::*;
#[test]
fn split_private_markers_never_escape_into_normal_prose() {
    let mut stream = Stream::default();
    assert_eq!(stream.push("Fact.\u{e200}cite\u{e202}turn0"), "Fact.");
    assert_eq!(
        stream.push("search0\u{e201} Next."),
        " [source unavailable] Next."
    );
    let literal = "`\u{e200}cite\u{e202}turn0\u{e201}`";
    assert_eq!(Stream::default().push(literal), literal);
    let literal = "``\u{e200}cite\u{e202}turn0\u{e201}``";
    assert_eq!(Stream::default().push(literal), literal);
}
