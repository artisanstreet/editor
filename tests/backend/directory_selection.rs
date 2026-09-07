//! Selected-directory authority coverage through the public Forge API.

use std::time::{Duration, Instant};

use artisan_backend::{
    DirectorySelectionAdmissionError, IssuedDirectory, MAX_LIFETIME_ISSUED_IDENTITIES,
    MAX_LIVE_SELECTIONS, SELECTION_TIME_TO_LIVE, SelectedDirectory, SelectedDirectoryAuthority,
};
use artisan_domain::{DirectoryId, DisplayNameError, RootPath};

/// Neutral lifecycle fixtures use forward slashes so the same text parses to
/// one final component on every supported platform.
const FIRST_ROOT: &str = "C:/repos/first-project";
const SECOND_ROOT: &str = "C:/repos/second-project";

const _: fn() = || {
    struct DebugMarker;
    trait AmbiguousIfDebug<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<DebugMarker> for T {}
    let _ = <SelectedDirectoryAuthority as AmbiguousIfDebug<_>>::marker;
    let _ = <IssuedDirectory as AmbiguousIfDebug<_>>::marker;
    let _ = <SelectedDirectory as AmbiguousIfDebug<_>>::marker;
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <SelectedDirectoryAuthority as AmbiguousIfClone<_>>::marker;
    let _ = <IssuedDirectory as AmbiguousIfClone<_>>::marker;
    let _ = <SelectedDirectory as AmbiguousIfClone<_>>::marker;
};

fn identity(tag: &str) -> DirectoryId {
    DirectoryId::parse(format!("directory-selection-{tag}")).expect("fixture identity is valid")
}

fn root(text: &str) -> RootPath {
    RootPath::parse(text).expect("fixture root path is valid")
}

/// Anchors each test's synthetic timeline exactly once; every deadline
/// decision under test is then driven purely by caller-supplied offsets from
/// this observation.
fn base_instant() -> Instant {
    Instant::now()
}

/// Finds the largest whole-second offset from `anchor` that still yields a
/// supported [`Instant`] using an at-most-64-iteration binary search across
/// the entire supported range; no unbounded probing occurs and every probe
/// goes through the same [`Instant::checked_add`] arithmetic production
/// uses.
fn largest_representable_second_offset(anchor: Instant) -> u64 {
    let mut low: u64 = 0;
    let mut high: u64 = u64::MAX;
    while low < high {
        let middle = low + (high - low) / 2;
        match anchor.checked_add(Duration::from_secs(middle)) {
            Some(_) => low = middle + 1,
            None => high = middle,
        }
    }
    // Zero is representable, so the smallest unsupported whole-second offset
    // is at least one and one second before it is the largest supported one.
    low - 1
}

/// One nanosecond before the approved deadline, through checked duration
/// arithmetic; the fixed budgets make the subtraction provably representable.
fn just_before_deadline() -> Duration {
    SELECTION_TIME_TO_LIVE
        .checked_sub(Duration::from_nanos(1))
        .expect("the approved 600-second deadline exceeds one nanosecond")
}

#[test]
fn consumed_selection_returns_exact_payload_once() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();
    let picked = identity("payload");

    let issued: IssuedDirectory = authority
        .register(picked.clone(), root(FIRST_ROOT), base)
        .expect("a fresh identity admits");
    assert_eq!(issued.directory_id, picked);
    assert_eq!(issued.root_path.as_str(), FIRST_ROOT);
    assert_eq!(issued.display_name.as_str(), "first-project");

    let selected: SelectedDirectory = authority
        .consume(&picked, base + Duration::from_secs(1))
        .expect("a live selection consumes exactly once");
    assert_eq!(selected.directory_id, picked);
    assert_eq!(selected.root_path.as_str(), FIRST_ROOT);
    assert_eq!(selected.display_name.as_str(), "first-project");

    assert!(
        authority
            .consume(&picked, base + Duration::from_secs(2))
            .is_none(),
        "the selection is single-use and answers unknown afterwards"
    );
}

#[test]
fn issued_identity_never_becomes_reusable_after_any_retirement() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    // A live collision fails without disturbing the live entry.
    let live = identity("history-live");
    authority
        .register(live.clone(), root(FIRST_ROOT), base)
        .expect("fresh live identity admits");
    assert!(matches!(
        authority.register(
            live.clone(),
            root(SECOND_ROOT),
            base + Duration::from_secs(1)
        ),
        Err(DirectorySelectionAdmissionError::IdentityAlreadyIssued)
    ));
    assert!(
        authority
            .consume(&live, base + Duration::from_secs(2))
            .is_some(),
        "the rejected collision left the live entry untouched"
    );

    // A consumed identity still collides.
    let consumed = identity("history-consumed");
    authority
        .register(consumed.clone(), root(FIRST_ROOT), base)
        .expect("fresh consumed identity admits");
    assert!(
        authority
            .consume(&consumed, base + Duration::from_secs(3))
            .is_some(),
        "the second fixture consumes cleanly"
    );
    assert!(matches!(
        authority.register(
            consumed.clone(),
            root(SECOND_ROOT),
            base + Duration::from_secs(4)
        ),
        Err(DirectorySelectionAdmissionError::IdentityAlreadyIssued)
    ));

    // An expired identity still collides.
    let expired = identity("history-expired");
    authority
        .register(expired.clone(), root(FIRST_ROOT), base)
        .expect("fresh expired identity admits");
    assert!(
        authority
            .consume(&expired, base + SELECTION_TIME_TO_LIVE)
            .is_none(),
        "the expired entry answers unknown at its deadline"
    );
    assert!(matches!(
        authority.register(
            expired.clone(),
            root(SECOND_ROOT),
            base + SELECTION_TIME_TO_LIVE
        ),
        Err(DirectorySelectionAdmissionError::IdentityAlreadyIssued)
    ));

    // None of the rejections corrupted admission for a genuinely fresh id.
    assert!(
        authority
            .register(
                identity("after-history"),
                root(FIRST_ROOT),
                base + SELECTION_TIME_TO_LIVE
            )
            .is_ok()
    );
}

#[test]
fn expiry_boundary_is_exact_and_unknown_answers_are_uniform() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    // One nanosecond before the deadline the selection is still consumable.
    let just_before = identity("boundary-before");
    authority
        .register(just_before.clone(), root(FIRST_ROOT), base)
        .expect("boundary fixture admits");
    let selected = authority
        .consume(&just_before, base + just_before_deadline())
        .expect("just before the deadline the selection is still live");
    assert_eq!(selected.directory_id, just_before);

    // Observed exactly at its deadline (`now >= expiry`) the entry has
    // already expired and prunes away instead of answering.
    let at_deadline = identity("boundary-equal");
    authority
        .register(at_deadline.clone(), root(SECOND_ROOT), base)
        .expect("second boundary fixture admits");
    assert!(
        authority
            .consume(&at_deadline, base + SELECTION_TIME_TO_LIVE)
            .is_none(),
        "equality with the deadline expires the entry"
    );

    // Never-registered, consumed, and expired identities all answer with the
    // same uniform unknown outcome.
    assert!(
        authority
            .consume(&identity("never-registered"), base + SELECTION_TIME_TO_LIVE)
            .is_none()
    );
}

#[test]
fn live_capacity_refuses_without_eviction_and_recovers_later() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    let seated: Vec<DirectoryId> = (0..MAX_LIVE_SELECTIONS)
        .map(|slot| identity(&format!("seat-{slot}")))
        .collect();
    for (slot, seat) in seated.iter().enumerate() {
        authority
            .register(
                seat.clone(),
                root(FIRST_ROOT),
                base + Duration::from_secs(slot as u64),
            )
            .expect("each seated identity admits while capacity remains");
    }

    let overflow = identity("seat-overflow");
    assert!(
        matches!(
            authority.register(
                overflow.clone(),
                root(SECOND_ROOT),
                base + Duration::from_secs(
                    u64::try_from(MAX_LIVE_SELECTIONS).expect("small const")
                ),
            ),
            Err(DirectorySelectionAdmissionError::LiveCapacityFull)
        ),
        "the ninth unexpired selection is refused"
    );

    // Nothing was evicted to make room: every seated identity still consumes.
    for seat in &seated {
        assert!(
            authority
                .consume(seat, base + Duration::from_secs(9))
                .is_some(),
            "the refused admission never evicted a seated selection"
        );
    }

    // Consumption returned capacity, so the previously refused identity admits.
    let readmitted = overflow;
    authority
        .register(
            readmitted.clone(),
            root(SECOND_ROOT),
            base + Duration::from_secs(10),
        )
        .expect("a consumed seat frees live capacity");

    // Expiry also returns capacity: past every deadline nothing stays live.
    let after_all_deadlines = base + SELECTION_TIME_TO_LIVE + Duration::from_secs(11);
    assert!(
        authority
            .consume(&readmitted, after_all_deadlines)
            .is_none()
    );
    assert!(
        authority
            .register(
                identity("seat-after-expiry"),
                root(FIRST_ROOT),
                after_all_deadlines
            )
            .is_ok()
    );
}

#[test]
fn lifetime_budget_stays_exhausted_with_no_live_state_left() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    for index in 0..MAX_LIFETIME_ISSUED_IDENTITIES {
        let one_shot = identity(&format!("burn-{index}"));
        authority
            .register(
                one_shot.clone(),
                root(FIRST_ROOT),
                base + Duration::from_secs((index % 8) as u64),
            )
            .expect("the budget admits while lifetime capacity remains");
        assert!(
            authority
                .consume(
                    &one_shot,
                    base + Duration::from_secs(((index % 8) as u64) + 1)
                )
                .is_some(),
            "each burned selection consumes immediately"
        );
    }

    // Every issued identity was consumed, so no live state remains.
    let drained = base + Duration::from_secs(9);
    let stranger = identity("burn-after-budget");
    assert!(matches!(
        authority.register(stranger.clone(), root(FIRST_ROOT), drained),
        Err(DirectorySelectionAdmissionError::LifetimeExhausted)
    ));

    // Even with zero live entries and every deadline long passed, lifetime
    // exhaustion stays permanent for this owner instance.
    let far_out = drained + SELECTION_TIME_TO_LIVE * 2;
    assert!(matches!(
        authority.register(identity("burn-after-deadlines"), root(FIRST_ROOT), far_out),
        Err(DirectorySelectionAdmissionError::LifetimeExhausted)
    ));
    assert!(authority.consume(&stranger, far_out).is_none());
}

#[test]
fn rejected_admissions_leave_an_unused_identity_unused() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();
    let survivor = identity("survivor");

    // The derived display name alone exceeds the 256-byte ceiling.
    let overlong_component = "overlong-".repeat(40);
    let overlong_failure = authority.register(
        survivor.clone(),
        root(&format!("C:/repos/{overlong_component}")),
        base,
    );
    assert!(
        matches!(
            overlong_failure,
            Err(DirectorySelectionAdmissionError::DisplayName(
                DisplayNameError::TooLong {
                    length: 360,
                    maximum: 256,
                }
            ))
        ),
        "an overlong derived name refuses admission with exact bounds"
    );

    // A blank-after-trim derived name refuses too.
    assert!(matches!(
        authority.register(survivor.clone(), root("C:/repos/   "), base),
        Err(DirectorySelectionAdmissionError::DisplayName(
            DisplayNameError::Blank
        ))
    ));

    // Neither rejection burned the otherwise unused identity.
    let issued: IssuedDirectory = authority
        .register(
            survivor.clone(),
            root(FIRST_ROOT),
            base + Duration::from_secs(1),
        )
        .expect("the surviving identity still admits after rejected attempts");
    assert_eq!(issued.directory_id, survivor);
}

#[test]
fn deadline_overflow_refuses_without_burning_the_identity() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();
    let doomed = identity("overflow");

    // A bounded construction of a supported observation at the monotonic
    // boundary: the at-most-64-iteration binary search finds the largest
    // representable whole-second offset, where a checked 600-second deadline
    // cannot exist.
    let boundary = base
        .checked_add(Duration::from_secs(largest_representable_second_offset(
            base,
        )))
        .expect("the searched whole-second offset is representable by construction");
    assert!(
        boundary.checked_add(SELECTION_TIME_TO_LIVE).is_none(),
        "the boundary observation must leave no room for the deadline"
    );

    let Err(failure) = authority.register(doomed.clone(), root(FIRST_ROOT), boundary) else {
        panic!("a deadline outside the monotonic range must refuse admission");
    };
    assert!(matches!(
        failure,
        DirectorySelectionAdmissionError::DeadlineOverflow
    ));

    // The refused admission burned neither the identity nor any other state:
    // the same identity admits normally at a representable observation.
    let issued: IssuedDirectory = authority
        .register(doomed.clone(), root(FIRST_ROOT), base)
        .expect("the identity survives an overflow refusal untouched");
    assert_eq!(issued.directory_id, doomed);
    assert!(
        authority
            .consume(&doomed, base + just_before_deadline())
            .is_some(),
        "the overflow refusal left no trace on later admission behavior"
    );
}

#[test]
fn derived_names_preserve_unicode_spacing_and_case_verbatim() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    let spaced = "C:/repos/Ærta Projekt ";
    let issued = authority
        .register(identity("verbatim-spacing"), root(spaced), base)
        .expect("a component with interior and trailing spaces admits");
    assert_eq!(
        issued.display_name.as_str(),
        "Ærta Projekt ",
        "spacing, case, and Unicode content survive verbatim without trimming or lossy conversion"
    );
}

#[test]
fn observations_never_extend_a_registered_deadline() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    let observed = identity("observed");
    authority
        .register(observed.clone(), root(FIRST_ROOT), base)
        .expect("observed fixture admits");

    // Interleaved registration and consumption activity happens on the same
    // authority while the first selection's clock keeps running.
    let bystander = identity("bystander");
    authority
        .register(
            bystander.clone(),
            root(SECOND_ROOT),
            base + Duration::from_secs(300),
        )
        .expect("bystander fixture admits mid-timeline");
    assert!(
        authority
            .consume(&bystander, base + Duration::from_secs(301))
            .is_some()
    );

    // The first selection kept its original deadline despite the activity:
    // observing and operating never advanced or extended it.
    assert!(
        authority
            .consume(&observed, base + SELECTION_TIME_TO_LIVE)
            .is_none(),
        "the original deadline held exactly despite interleaved operations"
    );
}

#[test]
fn approved_process_budgets_are_pinned_to_the_contracted_values() {
    assert_eq!(MAX_LIVE_SELECTIONS, 8);
    assert_eq!(MAX_LIFETIME_ISSUED_IDENTITIES, 256);
    assert_eq!(SELECTION_TIME_TO_LIVE, Duration::from_secs(600));
}

#[test]
fn typed_failures_carry_no_path_or_name_payload() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    let secret_component = format!("SECRET-{}", "payload-fragment".repeat(20));
    let Err(failure) = authority.register(
        identity("redact-name"),
        root(&format!("C:/repos/{secret_component}")),
        base,
    ) else {
        panic!("an overlong derived name must refuse admission");
    };

    let rendered_display = failure.to_string();
    let rendered_debug = format!("{failure:?}");
    assert!(!rendered_display.contains("SECRET"));
    assert!(!rendered_display.contains("payload-fragment"));
    assert!(!rendered_debug.contains("SECRET"));
    assert!(!rendered_debug.contains("payload-fragment"));
}

#[cfg(windows)]
#[test]
fn windows_canonical_root_forms_derive_expected_labels() {
    let base = base_instant();

    let cases: [(&str, &str); 8] = [
        (r"C:\", "C:"),
        (r"c:\", "c:"),
        (r"\\?\C:\", "C:"),
        (r"\\?\c:\", "c:"),
        (r"\\Server\Share\", "Share"),
        (r"\\?\UNC\server\share\", "share"),
        (r"C:\repos\Artisan Editor", "Artisan Editor"),
        (r"C:\repos\Ærta-Projekt", "Ærta-Projekt"),
    ];

    for (index, (candidate_root, expected_label)) in cases.into_iter().enumerate() {
        let mut authority = SelectedDirectoryAuthority::new();
        let issued = authority
            .register(
                identity(&format!("windows-form-{index}")),
                root(candidate_root),
                base,
            )
            .expect("canonical Windows forms admit");
        assert_eq!(
            issued.display_name.as_str(),
            expected_label,
            "case-sensitive derivation failed for form {index}"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_device_namespace_root_fails_closed_without_payload() {
    let base = base_instant();
    let mut authority = SelectedDirectoryAuthority::new();

    let Err(failure) = authority.register(
        identity("device-namespace"),
        root(r"\\.\PhysicalDrive0"),
        base,
    ) else {
        panic!("device-namespace roots must fail closed without a label");
    };

    assert!(matches!(
        failure,
        DirectorySelectionAdmissionError::UnnamedRootForm
    ));
    let rendered = format!("{failure}{failure:?}");
    assert!(!rendered.contains("PhysicalDrive0"));
}

#[cfg(unix)]
#[test]
fn unix_root_forms_derive_basename_and_filesystem_root_label() {
    let base = base_instant();

    let cases: [(&str, &str); 2] = [("/home/user/project", "project"), ("/", "/")];

    for (index, (candidate_root, expected_label)) in cases.into_iter().enumerate() {
        let mut authority = SelectedDirectoryAuthority::new();
        let issued = authority
            .register(
                identity(&format!("unix-form-{index}")),
                root(candidate_root),
                base,
            )
            .expect("canonical Unix forms admit");
        assert_eq!(
            issued.display_name.as_str(),
            expected_label,
            "derivation failed for Unix form {index}"
        );
    }
}
