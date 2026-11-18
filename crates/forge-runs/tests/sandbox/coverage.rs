use super::*;
use artisan_api::{NoticeKind, UnsandboxedReason};

fn unsandboxed(items: &[Item]) -> Vec<(UnsandboxedReason, String)> {
    items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Notice {
                kind: NoticeKind::Unsandboxed { reason },
                text,
            } = &item.kind
            {
                Some((*reason, text.inline().unwrap_or_default().to_owned()))
            } else {
                None
            }
        })
        .collect()
}

async fn set_permissions(fixture: &Fixture, permissions: Permissions) {
    let mut config = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap()
        .config;
    config.permissions = permissions;
    fixture
        .hub
        .update_thread(
            &fixture.thread_id,
            artisan_api::UpdateThread {
                config: Some(config),
                ..artisan_api::UpdateThread::default()
            },
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_auto_run_on_a_host_without_a_sandbox_says_so_and_full_access_does_not() {
    let fixture = fixture_on_host(
        Arc::new(|_turn, io| {
            Box::pin(async move {
                io.emit(reply("m1", "done")).await;
                Outcome::done()
            })
        }),
        sandbox::Host::Unavailable(forge_isolation::Unavailable::KernelWithoutLandlock),
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    fixture.settled().await;
    let notices = unsandboxed(&fixture.items().await);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].0, UnsandboxedReason::KernelWithoutLandlock);
    assert!(notices[0].1.contains("isn't sandboxed"), "{notices:?}");

    set_permissions(&fixture, Permissions::Full).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("more"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        unsandboxed(&fixture.items().await).len(),
        1,
        "Full access never had a sandbox to miss"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_switched_from_full_access_to_auto_says_it_finishes_outside_the_sandbox() {
    let fixture = fixture_on_host(
        Arc::new(|turn, mut io| {
            Box::pin(async move {
                io.emit(reply("m1", "working")).await;
                if turn.config.permissions == Permissions::Full {
                    match io.inbox.recv().await {
                        Some(Input::SetPermissions(Permissions::Auto)) => {}
                        other => return Outcome::failed(format!("unexpected {other:?}")),
                    }
                }
                Outcome::done()
            })
        }),
        sandbox::Host::Enforced,
    )
    .await;
    set_permissions(&fixture, Permissions::Full).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("work"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| texts(items).iter().any(|(text, _)| text == "working"))
        .await;
    set_permissions(&fixture, Permissions::Auto).await;
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status(), RunStatus::Done, "{:?}", runs[0].error());
    let notices = unsandboxed(&fixture.items().await);
    assert_eq!(
        notices
            .iter()
            .map(|(reason, _)| *reason)
            .collect::<Vec<_>>(),
        [UnsandboxedReason::StartedWithFullAccess],
        "{notices:?}"
    );

    fixture
        .hub
        .send_message(&fixture.thread_id, message("more"))
        .await
        .unwrap();
    fixture.settled().await;
    assert_eq!(
        unsandboxed(&fixture.items().await).len(),
        1,
        "a sandboxed Auto run has nothing to say"
    );
}
