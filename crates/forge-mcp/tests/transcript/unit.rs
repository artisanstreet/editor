#[test]
fn times_read_as_utc_dates() {
    assert_eq!(super::time(0), "1970-01-01 00:00 UTC");
    assert_eq!(super::time(1_792_066_918_056), "2026-10-15 12:21 UTC");
}
