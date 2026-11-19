use artisan_api::{ProjectId, ProjectRepository};
use editor_core::{CoreEvent, HostEvent};
use std::time::Duration;
use test_support::clock::ManualClock;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, empty_state};

fn branch(name: &str) -> ProjectRepository {
    ProjectRepository {
        branch: Some(name.to_owned()),
        host: Some("github".to_owned()),
        web_url: None,
        worktree: None,
    }
}

fn project() -> artisan_api::Project {
    artisan_api::Project {
        id: ProjectId("project".into()),
        path: "/work/project".into(),
        name: "project".into(),
        seq: 1,
        created_at: 0,
        removed: false,
    }
}

const REPOSITORY: &str = "/projects/project/repository";

fn next_read(forge: &mut StubbedHost, clock: &ManualClock, changed: &mut bool) -> HeldRequest {
    let project = ProjectId("project".into());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        forge.core.drain_with(|_, event| {
            if matches!(
                event,
                CoreEvent::Host {
                    event: HostEvent::Changed,
                    ..
                }
            ) {
                *changed = true;
            }
        });
        clock.advance(Duration::from_secs(3));
        forge.host_mut().load_repository(&project, false);
        if let Some(read) = forge.forge.try_take("GET", REPOSITORY) {
            return read;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the Editor never re-read the repository"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

#[test]
fn rereading_an_unchanged_repository_leaves_the_window_alone() {
    let clock = ManualClock::starting_at(0);
    let mut state = empty_state();
    state.projects.push(project());
    let mut forge = StubbedHost::start_with_clock(state, axum::Router::new(), clock.clock());
    let mut changed = false;
    let mut read = next_read(&mut forge, &clock, &mut changed);
    for (answer, shown, expected) in [
        (
            Some(branch("main")),
            "the first read shows the branch",
            true,
        ),
        (
            Some(branch("main")),
            "a periodic re-read of the same branch must not rebuild the window",
            false,
        ),
        (Some(branch("feature")), "a switched branch is shown", true),
        (None, "leaving git is shown", true),
    ] {
        read.respond(answer.clone());
        changed = false;
        read = next_read(&mut forge, &clock, &mut changed);
        assert_eq!(changed, expected, "{shown}");
        assert_eq!(
            forge.host().repository(&ProjectId("project".into())),
            answer.as_ref(),
            "{shown}"
        );
    }
}
