use editor_aqua::conversation::parked::Shelf;

#[test]
fn the_shelf_drops_the_least_recently_parked_entry_past_its_count() {
    let mut shelf = Shelf::new(2, 100);
    shelf.put("a", 1, 10);
    shelf.put("b", 2, 10);
    shelf.put("c", 3, 10);

    assert_eq!(shelf.take(&"a"), None);
    assert_eq!(shelf.take(&"b"), Some(2));
    assert_eq!(shelf.take(&"c"), Some(3));
    assert!(shelf.is_empty());
}

#[test]
fn the_shelf_drops_old_entries_until_it_fits_its_byte_budget() {
    let mut shelf = Shelf::new(8, 100);
    shelf.put("a", 1, 40);
    shelf.put("b", 2, 40);
    shelf.put("c", 3, 40);

    assert_eq!(shelf.len(), 2);
    assert_eq!(shelf.take(&"a"), None);
}

#[test]
fn the_shelf_refuses_an_entry_larger_than_its_whole_budget() {
    let mut shelf = Shelf::new(8, 100);
    shelf.put("a", 1, 40);
    shelf.put("huge", 2, 101);

    assert_eq!(shelf.take(&"huge"), None);
    assert_eq!(shelf.take(&"a"), Some(1));
}

#[test]
fn parking_the_same_key_again_replaces_it_and_makes_it_newest() {
    let mut shelf = Shelf::new(2, 100);
    shelf.put("a", 1, 10);
    shelf.put("b", 2, 10);
    shelf.put("a", 3, 10);
    shelf.put("c", 4, 10);

    assert_eq!(shelf.take(&"b"), None);
    assert_eq!(shelf.take(&"a"), Some(3));
}

#[test]
fn retain_forgets_the_entries_it_rejects() {
    let mut shelf = Shelf::new(8, 100);
    shelf.put(("host", 1), 1, 10);
    shelf.put(("other", 2), 2, 10);

    shelf.retain(|(host, _)| *host != "host");

    assert_eq!(shelf.take(&("host", 1)), None);
    assert_eq!(shelf.take(&("other", 2)), Some(2));
}
