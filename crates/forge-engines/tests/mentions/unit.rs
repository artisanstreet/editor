use super::*;
#[test]
fn a_mentioned_thread_reaches_the_engine_by_id_and_the_newest_title_wins() {
    let threads = [
        (ThreadId::from("new"), "Fix sidebar lag".to_owned()),
        (ThreadId::from("old"), "Fix sidebar lag".to_owned()),
    ];
    let text = with_thread_mentions("Like @Fix sidebar lag, twice: @Fix sidebar lag", &threads);
    assert!(
        text.ends_with(
            "(The user mentioned the Artisan thread \"Fix sidebar lag\" (`new`): read what this request needs with the artisan thread_read tool.)"
        ),
        "{text}"
    );
    assert_eq!(with_thread_mentions("mail me@Fix", &threads), "mail me@Fix");
}
