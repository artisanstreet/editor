use super::*;

fn session() -> Session {
    Session::new(
        ClaudeAiSession {
            cookies: vec![
                ("sessionKey".to_owned(), "abc".to_owned()),
                ("__cf_bm".to_owned(), "old".to_owned()),
            ],
            user_agent: "agent".to_owned(),
        },
        "org".to_owned(),
        Some("me@example.com".to_owned()),
    )
}

#[test]
fn a_session_round_trips_and_only_its_user_can_read_it() {
    let home = tempfile::tempdir().expect("a home");
    session().save(home.path()).expect("the session saves");
    let read = Session::load(home.path())
        .expect("the session loads")
        .expect("there is a session");
    assert_eq!(cookie(&read, "sessionKey").as_deref(), Some("abc"));
    assert_eq!(
        Session::state(home.path()),
        ClaudeAiState::SignedIn {
            account: Some("me@example.com".to_owned())
        }
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(Session::path(home.path()))
            .expect("the file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the session is the user's alone");
    }
}

#[test]
fn a_response_s_cookies_are_written_back() {
    let home = tempfile::tempdir().expect("a home");
    session().save(home.path()).expect("the session saves");
    let now = chrono::DateTime::from_timestamp_millis(artisan_api::now()).expect("now");
    let changes: Vec<_> = [
        "__cf_bm=new; HttpOnly; Path=/; Domain=claude.ai; Expires=Wed, 21 Oct 2099 18:54:34 GMT",
        "tracker=x; Domain=.example.com",
        "sessionKey=; Max-Age=0; Domain=.claude.ai",
        "routingHint=r1; Path=/",
    ]
    .into_iter()
    .filter_map(|header| cookie_change(header, now))
    .collect();
    Session::update(home.path(), |session| session.apply(&changes)).expect("the update");
    let read = Session::load(home.path())
        .expect("the session loads")
        .expect("there is a session");
    assert_eq!(cookie(&read, "__cf_bm").as_deref(), Some("new"));
    assert_eq!(cookie(&read, "routingHint").as_deref(), Some("r1"));
    assert_eq!(
        cookie(&read, "tracker").as_deref(),
        None,
        "another site's cookie is not kept"
    );
    assert_eq!(
        cookie(&read, "sessionKey").as_deref(),
        None,
        "a cleared cookie is dropped"
    );
}

#[test]
fn an_update_after_sign_out_does_not_bring_the_session_back() {
    let home = tempfile::tempdir().expect("a home");
    Session::update(home.path(), |session| session.needs_renewal = true).expect("no-op");
    assert_eq!(Session::state(home.path()), ClaudeAiState::SignedOut);
}

#[test]
fn a_debug_print_never_shows_a_cookie_value() {
    let printed = format!("{:?}", session());
    assert!(printed.contains("sessionKey"));
    assert!(!printed.contains("abc"));
}

fn cookie(session: &Session, name: &str) -> Option<String> {
    session.cookie_header().split("; ").find_map(|pair| {
        pair.strip_prefix(name)?
            .strip_prefix('=')
            .map(str::to_owned)
    })
}
