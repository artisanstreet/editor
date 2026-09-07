//! Focused, dependency-free coverage for product-telemetry capture policy.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/product_telemetry_capture_policy.rs"]
mod product_telemetry_capture_policy;

use product_telemetry_capture_policy::{
    AdapterCaptureObservation, CanonicalEventId, CaptureCompletion, CaptureDispatch,
    DecodedProductTelemetryEvent, DecoderObservation, ProductTelemetryCaptureInput,
    ProductTelemetryCaptureMode, ProductTelemetryCapturePolicy,
};

fn input(
    canonical_event_id: &str,
    decoder: DecoderObservation,
    adapter: AdapterCaptureObservation,
) -> ProductTelemetryCaptureInput {
    ProductTelemetryCaptureInput::new(canonical_event_id.into(), decoder, adapter)
}

fn live_capture(
    canonical_event_id: &str,
    decoder: DecoderObservation,
    adapter: AdapterCaptureObservation,
) -> product_telemetry_capture_policy::CaptureDecision {
    ProductTelemetryCapturePolicy::live().capture(input(canonical_event_id, decoder, adapter))
}

fn assert_no_dispatch(decision: &product_telemetry_capture_policy::CaptureDecision) {
    assert_eq!(decision.dispatch, CaptureDispatch::None);
    assert_eq!(decision.dispatch.count(), 0);
    assert!(!decision.dispatched_once());
    assert!(decision.dispatch.request().is_none());
    assert_eq!(decision.completion, CaptureCompletion::Succeeded);
}

#[test]
fn decoder_success_is_the_only_live_observation_that_dispatches() {
    let decoded = DecoderObservation::Decoded(DecodedProductTelemetryEvent::new());
    assert!(decoded.is_decoded());

    let decision = live_capture(
        "event:decoded",
        decoded,
        AdapterCaptureObservation::Succeeded,
    );
    assert!(decision.dispatched_once());
    assert_eq!(decision.dispatch.count(), 1);
    assert_eq!(decision.completion, CaptureCompletion::Succeeded);
    let request = decision.dispatch.request().expect("decoded event dispatch");
    assert_eq!(request.event(), DecodedProductTelemetryEvent::new());
    assert_eq!(request.canonical_event_id().as_str(), "event:decoded");
}

#[test]
fn invalid_and_failed_decoder_observations_are_silently_dropped() {
    for decoder in [DecoderObservation::Invalid, DecoderObservation::Failed] {
        assert!(!decoder.is_decoded());
        assert_no_dispatch(&live_capture(
            "event:undecodable",
            decoder,
            AdapterCaptureObservation::Succeeded,
        ));
    }
}

#[test]
fn no_op_mode_is_distinct_and_never_dispatches() {
    let noop = ProductTelemetryCapturePolicy::noop();
    assert_eq!(noop.mode(), ProductTelemetryCaptureMode::Noop);

    for decoder in [
        DecoderObservation::Decoded(DecodedProductTelemetryEvent::new()),
        DecoderObservation::Invalid,
        DecoderObservation::Failed,
    ] {
        for adapter in [
            AdapterCaptureObservation::Succeeded,
            AdapterCaptureObservation::OrdinaryFailure,
            AdapterCaptureObservation::ExceptionalCauseFailure,
        ] {
            assert_no_dispatch(&noop.capture(input("event:noop", decoder, adapter)));
        }
    }
}

#[test]
fn live_mode_is_explicit_and_preserves_exact_canonical_id_custody() {
    let policy = ProductTelemetryCapturePolicy::new(ProductTelemetryCaptureMode::Live);
    assert_eq!(policy.mode(), ProductTelemetryCaptureMode::Live);

    let canonical_event_id = "renderer_intent:0000-Δ/opaque?exact=true";
    let decision = policy.capture(input(
        canonical_event_id,
        DecoderObservation::decoded(),
        AdapterCaptureObservation::Succeeded,
    ));
    let CaptureDispatch::Once(request) = decision.dispatch else {
        panic!("valid decoded event must dispatch once");
    };
    assert_eq!(request.canonical_event_id().as_str(), canonical_event_id);
    assert_eq!(
        request.canonical_event_id().clone().into_inner(),
        canonical_event_id
    );
}

#[test]
fn every_adapter_observation_is_contained_after_one_valid_dispatch() {
    for adapter in [
        AdapterCaptureObservation::Succeeded,
        AdapterCaptureObservation::OrdinaryFailure,
        AdapterCaptureObservation::ExceptionalCauseFailure,
    ] {
        let decision = live_capture("event:adapter", DecoderObservation::decoded(), adapter);
        assert_eq!(decision.dispatch.count(), 1);
        assert_eq!(decision.completion, CaptureCompletion::Succeeded);
        assert!(decision.dispatch.request().is_some());
    }
}

#[test]
fn successful_ordinary_failure_and_cause_failure_have_equivalent_completion() {
    let successful = live_capture(
        "event:equivalent",
        DecoderObservation::decoded(),
        AdapterCaptureObservation::Succeeded,
    );
    let ordinary_failure = live_capture(
        "event:equivalent",
        DecoderObservation::decoded(),
        AdapterCaptureObservation::OrdinaryFailure,
    );
    let cause_failure = live_capture(
        "event:equivalent",
        DecoderObservation::decoded(),
        AdapterCaptureObservation::ExceptionalCauseFailure,
    );

    assert_eq!(successful, ordinary_failure);
    assert_eq!(ordinary_failure, cause_failure);
    assert_eq!(successful.completion, CaptureCompletion::Succeeded);
}

#[test]
fn adapter_observations_cannot_make_decode_failure_dispatch() {
    for decoder in [DecoderObservation::Invalid, DecoderObservation::Failed] {
        for adapter in [
            AdapterCaptureObservation::Succeeded,
            AdapterCaptureObservation::OrdinaryFailure,
            AdapterCaptureObservation::ExceptionalCauseFailure,
        ] {
            assert_no_dispatch(&live_capture("event:drop", decoder, adapter));
        }
    }
}

#[test]
fn repeated_independent_live_captures_each_dispatch_once_with_its_own_id() {
    let policy = ProductTelemetryCapturePolicy::live();
    let ids = ["event:first", "event:second", "event:third"];

    for id in ids {
        let decision = policy.capture(input(
            id,
            DecoderObservation::decoded(),
            AdapterCaptureObservation::ExceptionalCauseFailure,
        ));
        assert_eq!(decision.dispatch.count(), 1);
        assert!(decision.dispatched_once());
        assert_eq!(
            decision
                .dispatch
                .request()
                .expect("one dispatch")
                .canonical_event_id()
                .as_str(),
            id
        );
    }
}

#[test]
fn canonical_event_id_conversions_retain_the_supplied_spelling() {
    let from_string = CanonicalEventId::from(String::from("event:string"));
    let from_str = CanonicalEventId::from("event:str");

    assert_eq!(from_string.as_str(), "event:string");
    assert_eq!(from_string.into_inner(), "event:string");
    assert_eq!(from_str.as_str(), "event:str");
}
