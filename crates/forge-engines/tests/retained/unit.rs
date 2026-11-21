use super::{RETIRED_IDS, Retired};

#[test]
fn a_long_run_forgets_its_oldest_settled_ids_and_keeps_the_newest() {
    let mut retired = Retired::default();
    for n in 0..10_000 {
        retired.insert(format!("t{n}"));
    }
    assert!(
        !retired.contains("t0"),
        "the oldest id is forgotten once the record is full"
    );
    assert!(
        !retired.contains(&format!("t{}", 10_000 - RETIRED_IDS - 1)),
        "every id older than the bound is forgotten"
    );
    assert!(
        retired.contains(&format!("t{}", 10_000 - RETIRED_IDS)),
        "the oldest id inside the bound is kept"
    );
    assert!(retired.contains("t9999"), "the newest id is kept");
}
