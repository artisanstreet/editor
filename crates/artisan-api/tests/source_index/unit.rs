use super::*;
#[test]
fn source_interpretation_is_explicit_on_the_wire() {
    let query: SourceIndexQuery =
        serde_json::from_str(r#"{"format":"literal","offset":null,"limit":64}"#).unwrap();
    assert_eq!(query.format, SourceIndexFormat::Literal);
    serde_json::from_str::<SourceIndexQuery>(r#"{"offset":null,"limit":64}"#)
        .expect_err("a query without a format is refused");
    serde_json::from_str::<SourceIndexQuery>(r#"{"format":"ansi","offset":null,"limit":64}"#)
        .expect_err("an unknown format is refused");
    assert_eq!(SourceIndexFormat::Markdown.as_str(), "markdown");
}
