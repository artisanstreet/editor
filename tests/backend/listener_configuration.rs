//! Private `cfg(test)` proof for the approved pending-peer configuration.
//!
//! Linked from the listener module through the `#[path]` attribute, this file
//! exercises the REAL production helper that `ForgeListener::bind` calls. It
//! captures the public transport/crypto `Arc` owners of the caller-selected
//! `ServerConfig`, asserts `Arc::ptr_eq` across the helper for both, checks
//! the pinned `Debug` rendering reports the approved `8 / 65_536 / 524_288`
//! pending-peer values after deliberately different inputs, verifies all other
//! rendered settings survive byte-for-byte (the public `transport` and
//! `crypto` Arc fields are captured and pointer-compared directly), and proves
//! the helper idempotent. This is production configuration wiring — never
//! network-backlog memory.

use super::*;
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::sync::Arc;

/// Builds one real ephemeral PKI-backed server configuration through the
/// existing transport constructor, then applies deliberately different
/// pending-peer values so the approved override is observable.
fn deliberate_pending_config() -> ServerConfig {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SANs");
    let certificate: CertificateDer<'static> = certified_key.cert.der().clone();
    let private_key = PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der());
    let mut config = artisan_transport::server_config(vec![certificate], private_key)
        .expect("caller server configuration");
    config.max_incoming(3);
    config.incoming_buffer_size(1111);
    config.incoming_buffer_size_total(222_333);
    config
}

/// Extracts the rendered value of one pending-peer field from the real
/// `ServerConfig` `Debug` output. The anchored `field:` form means
/// `incoming_buffer_size` never matches `_total`.
fn rendered_field(debug: &str, field: &str) -> Option<String> {
    let needle = format!("{field}:");
    let start = debug.find(&needle)?;
    let value = debug[start + needle.len()..].trim_start();
    let end = value.find([',', '}']).unwrap_or(value.len());
    Some(value[..end].trim().to_owned())
}

/// Removes one rendered `field: value` segment (including its trailing
/// comma when present) so two configurations compare equal over everything
/// except the three approved mutations. Removal positions are deterministic,
/// which is all this comparison needs.
fn without_rendered_field(debug: &str, field: &str) -> String {
    let needle = format!("{field}:");
    let Some(start) = debug.find(&needle) else {
        return debug.to_owned();
    };
    let rest = &debug[start + needle.len()..];
    let leading_ws = rest.len() - rest.trim_start().len();
    let value = rest[leading_ws..].trim_start();
    let value_len = value.find([',', '}']).unwrap_or(value.len());
    let mut end = start + needle.len() + leading_ws + value_len;
    let mut output = String::with_capacity(debug.len());
    output.push_str(&debug[..start]);
    if debug[end..].starts_with(',') {
        end += 1;
    } else if output.ends_with(',') {
        output.pop();
    }
    output.push_str(&debug[end..]);
    output
}

const APPROVED_FIELDS: [&str; 3] = [
    "max_incoming",
    "incoming_buffer_size",
    "incoming_buffer_size_total",
];

const DELIBERATE_VALUES: [&str; 3] = ["3", "1111", "222333"];
const APPROVED_VALUES: [&str; 3] = ["8", "65536", "524288"];

#[test]
fn approved_pending_values_overwrite_deliberate_inputs_and_preserve_the_rest() {
    let original = deliberate_pending_config();

    // Capture the REAL public Arc owners before the production helper runs.
    let transport_before = Arc::clone(&original.transport);
    let crypto_before = Arc::clone(&original.crypto);
    let before = format!("{original:?}");

    let bounded = apply_approved_pending_peer_limits(original);
    let transport_after = Arc::clone(&bounded.transport);
    let crypto_after = Arc::clone(&bounded.crypto);
    let after = format!("{bounded:?}");

    // Pointer identity of both caller-selected Arc owners across exactly the
    // three approved pending-peer mutations.
    assert!(
        Arc::ptr_eq(&transport_before, &transport_after),
        "transport configuration Arc identity must be preserved"
    );
    assert!(
        Arc::ptr_eq(&crypto_before, &crypto_after),
        "crypto configuration Arc identity must be preserved"
    );
    // Pointer identity of internal Arcs has no public observation point
    // (setter-only transport API); preservation is proven by the identical
    // stripped rendering below plus the helper's three-mutation scope.

    for (index, field) in APPROVED_FIELDS.into_iter().enumerate() {
        assert_eq!(
            rendered_field(&before, field).as_deref(),
            Some(DELIBERATE_VALUES[index]),
            "deliberate input must be observable for {field}"
        );
        assert_eq!(
            rendered_field(&after, field).as_deref(),
            Some(APPROVED_VALUES[index]),
            "approved override must be configured for {field}"
        );
    }

    let before_stripped = APPROVED_FIELDS
        .into_iter()
        .fold(before.clone(), |text, field| {
            without_rendered_field(&text, field)
        });
    let after_stripped = APPROVED_FIELDS
        .into_iter()
        .fold(after.clone(), |text, field| {
            without_rendered_field(&text, field)
        });
    assert_eq!(
        before_stripped, after_stripped,
        "rendering outside the three pending fields must be preserved"
    );

    // Reapplying the helper is stable; production bind is its sole caller.
    let reapplied = format!("{:?}", apply_approved_pending_peer_limits(bounded));
    assert_eq!(after, reapplied, "helper application must be idempotent");
}
