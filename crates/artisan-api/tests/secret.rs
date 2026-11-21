use artisan_api::Secret;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Credentials {
    client_id: String,
    client_secret: Secret,
}

#[test]
fn diagnostics_of_a_credential_never_print_it_but_storage_keeps_it() {
    let stored = r#"{"client_id":"artisan","client_secret":"s3cr3t-value"}"#;
    let credentials: Credentials = serde_json::from_str(stored).unwrap();
    let printed = format!("{credentials:?}");
    assert!(printed.contains("artisan"));
    assert!(!printed.contains("s3cr3t-value"));
    assert_eq!(credentials.client_secret.expose(), "s3cr3t-value");
    assert_eq!(serde_json::to_string(&credentials).unwrap(), stored);
}
