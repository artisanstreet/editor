//! Single-use credential authority coverage through the public Forge API.

use artisan_backend::{
    AuthenticatedCredential, CredentialAuthenticationError, CredentialAuthority, CredentialKind,
    PendingReconnect, ReconnectRotationError,
};
use artisan_protocol::{
    HelloCredential, LocalCapability, RECONNECT_CAPABILITY_BYTES, ReconnectCapability,
};

const INITIAL: [u8; 32] = [0x31; 32];
const OTHER_INITIAL: [u8; 32] = [0x62; 32];
const RECONNECT: [u8; RECONNECT_CAPABILITY_BYTES] = [0xa7; RECONNECT_CAPABILITY_BYTES];

const _: fn() = || {
    struct DebugMarker;
    trait AmbiguousIfDebug<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<DebugMarker> for T {}
    let _ = <CredentialAuthority as AmbiguousIfDebug<_>>::marker;
    let _ = <AuthenticatedCredential<'static> as AmbiguousIfDebug<_>>::marker;
    let _ = <PendingReconnect<'static> as AmbiguousIfDebug<_>>::marker;
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <CredentialAuthority as AmbiguousIfClone<_>>::marker;
    let _ = <AuthenticatedCredential<'static> as AmbiguousIfClone<_>>::marker;
    let _ = <PendingReconnect<'static> as AmbiguousIfClone<_>>::marker;
};

fn initial(bytes: [u8; 32]) -> HelloCredential {
    HelloCredential::Initial(LocalCapability::from_bytes(bytes))
}

fn reconnect(bytes: [u8; RECONNECT_CAPABILITY_BYTES]) -> HelloCredential {
    HelloCredential::Reconnect(ReconnectCapability::from_bytes(bytes))
}

#[test]
fn wrong_family_and_value_do_not_consume_the_expected_initial_credential() {
    let mut authority = CredentialAuthority::new(LocalCapability::from_bytes(INITIAL));

    assert!(matches!(
        authority.authenticate(reconnect(INITIAL)),
        Err(CredentialAuthenticationError::FamilyMismatch {
            expected: CredentialKind::Initial,
            presented: CredentialKind::Reconnect,
        })
    ));
    assert!(matches!(
        authority.authenticate(initial(OTHER_INITIAL)),
        Err(CredentialAuthenticationError::Rejected {
            kind: CredentialKind::Initial,
        })
    ));

    let accepted = authority.authenticate(initial(INITIAL));
    assert!(accepted.is_ok());
}

#[test]
fn accepted_credential_is_single_use_until_rotation_commits() {
    let mut authority = CredentialAuthority::new(LocalCapability::from_bytes(INITIAL));
    {
        let _grant = authority
            .authenticate(initial(INITIAL))
            .expect("matching initial capability authenticates");
    }

    assert!(matches!(
        authority.authenticate(initial(INITIAL)),
        Err(CredentialAuthenticationError::AwaitingRotation)
    ));
}

#[test]
fn rotation_scrubs_source_and_installs_only_after_welcome_take() {
    let mut authority = CredentialAuthority::new(LocalCapability::from_bytes(INITIAL));
    let grant = authority
        .authenticate(initial(INITIAL))
        .expect("matching initial capability authenticates");
    let mut material = RECONNECT;
    let mut pending = grant.prepare_reconnect(&mut material);
    assert_eq!(material, [0; RECONNECT_CAPABILITY_BYTES]);

    let delivered = pending
        .take_for_welcome()
        .expect("first Welcome take succeeds");
    assert!(delivered.constant_time_eq(&ReconnectCapability::from_bytes(RECONNECT)));
    assert!(matches!(
        pending.take_for_welcome(),
        Err(ReconnectRotationError::AlreadyTaken)
    ));
    pending.commit().expect("delivered rotation commits");

    let reconnect_grant = authority.authenticate(reconnect(RECONNECT));
    assert!(reconnect_grant.is_ok());
}

#[test]
fn rotation_cannot_commit_before_the_welcome_copy_is_taken() {
    let mut authority = CredentialAuthority::new(LocalCapability::from_bytes(INITIAL));
    let grant = authority
        .authenticate(initial(INITIAL))
        .expect("matching initial capability authenticates");
    let mut material = RECONNECT;
    let pending = grant.prepare_reconnect(&mut material);

    assert!(matches!(
        pending.commit(),
        Err(ReconnectRotationError::NotTakenForWelcome)
    ));
    assert!(matches!(
        authority.authenticate(reconnect(RECONNECT)),
        Err(CredentialAuthenticationError::AwaitingRotation)
    ));
}
