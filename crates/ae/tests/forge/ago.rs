use super::ago;

const SECOND: i64 = 1000;
const NOW: i64 = 1_000_000_000 * SECOND;

fn said(seconds_before: i64) -> String {
    ago(NOW, NOW - seconds_before * SECOND)
}

#[test]
fn every_unit_reads_as_english_in_the_singular_and_the_plural() {
    assert_eq!(said(0), "just now");
    assert_eq!(said(59), "just now");
    assert_eq!(said(60), "1 min ago");
    assert_eq!(said(150), "2 min ago");
    assert_eq!(said(3600), "1 h ago");
    assert_eq!(said(7300), "2 h ago");
    assert_eq!(said(86_400), "1 day ago");
    assert_eq!(said(2 * 86_400 + 5), "2 days ago");
    assert_eq!(said(-30), "just now");
}
