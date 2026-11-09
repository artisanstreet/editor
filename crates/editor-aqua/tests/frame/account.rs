use editor_aqua::frame::account::Identity;

#[test]
fn username_replaces_email_without_replacing_the_display_name() {
    assert_eq!(
        Identity::of(" Sander ", "sander@example.com", Some(" ss ")),
        Identity::Named {
            name: "Sander".into(),
            detail: "@ss".into()
        }
    );
    assert_eq!(
        Identity::of("", "sander@example.com", Some("@ss")),
        Identity::Unnamed {
            detail: "@ss".into()
        }
    );
    for username in [None, Some(""), Some("  ")] {
        assert_eq!(
            Identity::of("Sander", "sander@example.com", username),
            Identity::Named {
                name: "Sander".into(),
                detail: "sander@example.com".into()
            }
        );
    }
}
