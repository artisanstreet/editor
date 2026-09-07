//! Dependency-free coverage for the shared image-inspection lease counter.

#[path = "../../modules/frontend/src/image_inspection_store.rs"]
mod image_inspection_store;

use image_inspection_store::ImageInspectionStore;

#[test]
fn zero_and_default_states_are_hidden() {
    let new = ImageInspectionStore::new();
    let default = ImageInspectionStore::default();

    assert_eq!(new.count(), 0);
    assert!(!new.is_visible());
    assert_eq!(default.count(), 0);
    assert!(!default.is_visible());
}

#[test]
fn overlapping_leases_keep_visibility_until_the_last_release() {
    let mut store = ImageInspectionStore::default();

    store.retain();
    store.retain();
    assert_eq!(store.count(), 2);
    assert!(store.is_visible());

    store.release();
    assert_eq!(store.count(), 1);
    assert!(store.is_visible());

    store.release();
    assert_eq!(store.count(), 0);
    assert!(!store.is_visible());
}

#[test]
fn premature_release_does_not_hide_another_viewer() {
    let mut store = ImageInspectionStore::default();

    store.retain();
    store.retain();
    store.release();

    assert_eq!(store.count(), 1);
    assert!(store.is_visible());
}

#[test]
fn release_saturates_at_zero() {
    let mut store = ImageInspectionStore::default();

    store.release();
    store.release();
    assert_eq!(store.count(), 0);
    assert!(!store.is_visible());

    store.retain();
    store.release();
    store.release();
    assert_eq!(store.count(), 0);
    assert!(!store.is_visible());
}

#[test]
fn visibility_transitions_follow_count_exactly() {
    let mut store = ImageInspectionStore::new();
    let transitions = [(0, false), (1, true), (2, true), (1, true), (0, false)];

    assert_eq!((store.count(), store.is_visible()), transitions[0]);
    store.retain();
    assert_eq!((store.count(), store.is_visible()), transitions[1]);
    store.retain();
    assert_eq!((store.count(), store.is_visible()), transitions[2]);
    store.release();
    assert_eq!((store.count(), store.is_visible()), transitions[3]);
    store.release();
    assert_eq!((store.count(), store.is_visible()), transitions[4]);
}
