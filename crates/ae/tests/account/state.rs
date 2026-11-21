use super::*;

fn enrolled() -> Account {
    Account::Enrolled {
        key: MachineKeypair::generate().unwrap(),
        machine_id: "mch_1".to_owned(),
        account_id: "acct_1".to_owned(),
    }
}

#[test]
fn a_saved_account_loads_back_and_stays_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data/ast_account.json");
    let account = enrolled();

    save_account(&path, &account).unwrap();

    assert_eq!(load_account(&path).unwrap(), account);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["ast_account.json"]);
}

#[cfg(unix)]
#[test]
fn a_save_that_cannot_finish_keeps_the_previous_key() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ast_account.json");
    let account = enrolled();
    save_account(&path, &account).unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

    let replaced = save_account(
        &path,
        &Account::Keyed {
            key: MachineKeypair::generate().unwrap(),
        },
    );

    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(replaced, Err(AccountFileError::Write { .. })),
        "{replaced:?}"
    );
    assert_eq!(load_account(&path).unwrap(), account);
}

#[test]
fn an_enrollment_without_its_key_is_not_reported_as_enrolled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ast_account.json");
    std::fs::write(&path, r#"{"machine_id":"mch_1","account_id":"acct_1"}"#).unwrap();

    let loaded = load_account(&path);

    assert!(
        matches!(&loaded, Err(AccountFileError::Decode { source: MachineFileError::Inconsistent { fields }, .. }) if fields == "machine_id, account_id"),
        "{loaded:?}"
    );
}

#[test]
fn a_key_that_does_not_match_its_secret_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ast_account.json");
    let mut file: serde_json::Value = serde_json::from_slice(&enrolled().encode()).unwrap();
    file["public_b64u"] = MachineKeypair::generate().unwrap().public_b64u().into();
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

    let loaded = load_account(&path);

    assert!(
        matches!(
            loaded,
            Err(AccountFileError::Decode {
                source: MachineFileError::Key(artisan_account::KeyProblem::Mismatch),
                ..
            })
        ),
        "{loaded:?}"
    );
}

#[test]
fn labels_fall_back_to_a_bounded_hostname() {
    assert_eq!(label_default(Some("  srv-1  ")), "srv-1");
    assert!(label_default(Some("")).is_empty());
    assert!(label_default(None).len() <= 64);
}

#[test]
fn expected_account_gate_rejects_mismatch_before_rpc() {
    check_expected_account(None, None).unwrap();
    check_expected_account(None, Some("acct_a")).unwrap();
    check_expected_account(Some("acct_a"), Some("acct_a")).unwrap();
    check_expected_account(Some(" acct_a "), Some("acct_a")).unwrap();
    assert!(check_expected_account(Some("acct_a"), Some("acct_b")).is_err());
    assert!(check_expected_account(Some("acct_a"), None).is_err());
    assert!(check_expected_account(Some(""), Some("acct_a")).is_err());
    assert!(check_expected_account(Some("   "), Some("acct_a")).is_err());
}

fn sorted_keys(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort_unstable();
    keys
}

#[test]
fn status_json_reports_ids_with_exact_keys() {
    let enrolled = enrolled_report("mch_1", Some("acct_1"));
    assert_eq!(enrolled["enrolled"], true);
    assert_eq!(enrolled["machine_id"], "mch_1");
    assert_eq!(enrolled["account_id"], "acct_1");
    assert_eq!(
        sorted_keys(&enrolled),
        ["account_id", "enrolled", "machine_id"]
    );
    let pending = pending_report(
        "enr_1",
        &artisan_account::PairStatus {
            enrollment_id: None,
            status: "claimed".to_owned(),
            account_id: Some("acct_9".to_owned()),
            email: Some("op@example.invalid".to_owned()),
            display_name: None,
            expires_at: None,
        },
    );
    assert_eq!(pending["enrolled"], false);
    assert_eq!(pending["enrollment_id"], "enr_1");
    assert_eq!(pending["status"], "claimed");
    assert_eq!(pending["account_id"], "acct_9");
    assert_eq!(pending["email"], "op@example.invalid");
    assert!(pending["display_name"].is_null());
    assert_eq!(
        sorted_keys(&pending),
        [
            "account_id",
            "display_name",
            "email",
            "enrolled",
            "enrollment_id",
            "status"
        ]
    );
}

#[test]
fn status_json_never_carries_secrets() {
    let secret = "SECRET-B64U-abcdefghijklmnopqrstuvwxyz012345";
    let public = "PUBLIC-B64U-abcdefghijklmnopqrstuvwxyz012345";
    let enrolled = enrolled_report("mch_1", Some("acct_1"));
    let pending = pending_report(
        "enr_1",
        &artisan_account::PairStatus {
            enrollment_id: None,
            status: "pending".to_owned(),
            account_id: None,
            email: None,
            display_name: None,
            expires_at: None,
        },
    );
    for report in [
        serde_json::to_string(&enrolled).unwrap(),
        serde_json::to_string(&pending).unwrap(),
    ] {
        assert!(!report.contains(secret));
        assert!(!report.contains(public));
        assert!(!report.contains("secret"));
        assert!(!report.contains("token"));
        assert!(!report.contains("code"));
    }
}

#[test]
fn enroll_report_carries_the_created_dto_fields() {
    let report = enroll_report(&forge_http::core::EnrollmentCreated {
        enrollment_id: "enr_1".to_owned(),
        code: "ABCDEFGH".to_owned(),
        expires_at: "2030-01-01T00:00:00.000Z".to_owned(),
        key_fingerprint: "fp".to_owned(),
    });
    assert_eq!(report["code"], "ABCDEFGH");
    assert_eq!(report["enrollment_id"], "enr_1");
    assert_eq!(report["expires_at"], "2030-01-01T00:00:00.000Z");
    assert_eq!(report["key_fingerprint"], "fp");
    assert_eq!(
        sorted_keys(&report),
        ["code", "enrollment_id", "expires_at", "key_fingerprint"]
    );
}

#[test]
fn no_enrollment_json_uses_pending_keys_with_nulls() {
    let report = no_enrollment_report();
    assert_eq!(report["enrolled"], false);
    assert!(report["enrollment_id"].is_null());
    assert_eq!(report["status"], "none");
    assert!(report["account_id"].is_null());
    assert!(report["email"].is_null());
    assert!(report["display_name"].is_null());
    assert_eq!(
        sorted_keys(&report),
        [
            "account_id",
            "display_name",
            "email",
            "enrolled",
            "enrollment_id",
            "status"
        ]
    );
}
