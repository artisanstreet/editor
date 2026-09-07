//! Boundary and transition coverage for the pure engine-usage cache policy.

#[path = "../../modules/frontend/src/engine_usage_cache.rs"]
mod engine_usage_cache;

use engine_usage_cache::{
    ENGINE_USAGE_CACHE_STORAGE_KEY, ENGINE_USAGE_REFRESH_WINDOW_MS,
    EngineUsageCacheLoadObservation, EngineUsageCacheLoadOutput, EngineUsageCacheReadFailure,
    EngineUsageCacheSaveObservation, EngineUsageCacheSaveOutput, EngineUsageFreshness,
    engine_usage_cache_freshness, engine_usage_cache_load, engine_usage_cache_save,
    engine_usage_refresh_is_due,
};

const NOW_MS: i64 = 1_700_000_000_000;

#[test]
fn storage_key_and_refresh_window_are_exact() {
    assert_eq!(ENGINE_USAGE_CACHE_STORAGE_KEY, "artisan.engine-usage-cache");
    assert_eq!(ENGINE_USAGE_REFRESH_WINDOW_MS, 180_000);
}

#[test]
fn missing_and_invalid_parsed_timestamps_are_due() {
    // Timestamp parsing is outside the module; both an absent timestamp and a
    // parser rejection arrive as None.
    assert_eq!(
        engine_usage_cache_freshness(None, NOW_MS),
        EngineUsageFreshness::MissingOrInvalid
    );
    assert!(engine_usage_refresh_is_due(None, NOW_MS));
}

#[test]
fn freshness_changes_at_the_exact_threshold() {
    let cases = [
        (
            ENGINE_USAGE_REFRESH_WINDOW_MS - 1,
            EngineUsageFreshness::Fresh,
        ),
        (ENGINE_USAGE_REFRESH_WINDOW_MS, EngineUsageFreshness::Due),
        (
            ENGINE_USAGE_REFRESH_WINDOW_MS + 1,
            EngineUsageFreshness::Due,
        ),
    ];

    for (age_ms, expected) in cases {
        let fetched_at_ms = NOW_MS - age_ms;
        assert_eq!(
            engine_usage_cache_freshness(Some(fetched_at_ms), NOW_MS),
            expected,
            "age_ms={age_ms}"
        );
        assert_eq!(
            engine_usage_refresh_is_due(Some(fetched_at_ms), NOW_MS),
            expected.is_due(),
            "age_ms={age_ms}"
        );
    }
}

#[test]
fn equality_and_future_timestamps_stay_true_to_typescript_arithmetic() {
    assert!(!engine_usage_refresh_is_due(Some(NOW_MS), NOW_MS));
    assert!(!engine_usage_refresh_is_due(Some(NOW_MS + 1), NOW_MS));
    assert_eq!(
        engine_usage_cache_freshness(Some(NOW_MS + 1), NOW_MS),
        EngineUsageFreshness::Fresh
    );
}

#[test]
fn signed_extremes_do_not_wrap_age_calculation() {
    assert_eq!(
        engine_usage_cache_freshness(Some(i64::MIN), i64::MAX),
        EngineUsageFreshness::Due
    );
    assert_eq!(
        engine_usage_cache_freshness(Some(i64::MAX), i64::MIN),
        EngineUsageFreshness::Fresh
    );
    assert!(!engine_usage_refresh_is_due(Some(i64::MIN), i64::MIN));
    assert!(!engine_usage_refresh_is_due(Some(i64::MAX), i64::MAX));
}

#[test]
fn storage_and_schema_read_failures_request_removal_and_return_none() {
    for failure in [
        EngineUsageCacheReadFailure::Storage,
        EngineUsageCacheReadFailure::Schema,
    ] {
        let output =
            engine_usage_cache_load::<u8>(EngineUsageCacheLoadObservation::ReadFailure(failure));
        assert_eq!(output, EngineUsageCacheLoadOutput::RemoveCorruptEntry);
        assert!(output.requests_corrupt_entry_removal());
        assert_eq!(output.as_value(), None);
        assert_eq!(output.into_value(), None);
    }
}

#[test]
fn missing_load_returns_none_without_removal() {
    let output = engine_usage_cache_load::<u8>(EngineUsageCacheLoadObservation::Missing);

    assert_eq!(output, EngineUsageCacheLoadOutput::Missing);
    assert!(!output.requests_corrupt_entry_removal());
    assert_eq!(output.as_value(), None);
    assert_eq!(output.into_value(), None);
}

#[test]
fn valid_load_passes_the_value_through_unchanged() {
    let value = String::from("cached snapshot");
    let output = engine_usage_cache_load(EngineUsageCacheLoadObservation::Valid(value.clone()));

    assert_eq!(output, EngineUsageCacheLoadOutput::Valid(value.clone()));
    assert!(!output.requests_corrupt_entry_removal());
    assert_eq!(output.as_value(), Some(&value));
    assert_eq!(output.into_value(), Some(value));
}

#[test]
fn save_success_is_saved_and_save_failure_is_absorbed() {
    assert_eq!(
        engine_usage_cache_save(EngineUsageCacheSaveObservation::Succeeded),
        EngineUsageCacheSaveOutput::Saved
    );

    let failure = engine_usage_cache_save(EngineUsageCacheSaveObservation::Failed);
    assert_eq!(failure, EngineUsageCacheSaveOutput::FailureAbsorbed);
    assert!(failure.is_failure_absorbed());
}
