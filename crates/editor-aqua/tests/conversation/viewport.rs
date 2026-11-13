use editor_aqua::conversation::viewport::{Effect, Fetch, Input, Jump, State, Viewport};

#[expect(
    clippy::needless_pass_by_value,
    reason = "tests read as a sequence of inputs handed to the viewport"
)]
fn send(viewport: &mut Viewport, input: Input) -> Vec<Effect> {
    let mut effects = Vec::new();
    viewport.handle_with_context(&input, &mut effects);
    effects
}

#[test]
fn reading_preserves_anchor_and_fetches_a_gap_once() {
    let mut viewport = Viewport::default();
    assert_eq!(
        send(&mut viewport, Input::Open),
        [Effect::Fetch(Fetch::Tail)]
    );
    assert_eq!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Tail,
                first: Some(100),
                last: Some(199),
                more: true,
            }
        ),
        [Effect::ScrollToEnd]
    );
    assert!(
        send(
            &mut viewport,
            Input::ScrolledAway {
                anchor: 150,
                through: 160
            }
        )
        .is_empty()
    );
    assert_eq!(
        send(&mut viewport, Input::TopReached),
        [Effect::Fetch(Fetch::Older { before: 100 })]
    );
    assert!(send(&mut viewport, Input::TopReached).is_empty());
    assert!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::At { position: 75 },
                first: Some(75),
                last: Some(174),
                more: true,
            }
        )
        .is_empty()
    );
    assert_eq!((viewport.oldest, viewport.gap), (Some(75), Some(174)));
    assert_eq!(
        viewport.state(),
        &State::Reading {
            anchor: 150,
            through: 160
        }
    );
    assert!(send(&mut viewport, Input::EvictedNewer { last: 175 }).is_empty());
    assert_eq!(
        send(&mut viewport, Input::BottomReached),
        [Effect::Fetch(Fetch::Newer { after: 175 })]
    );
    assert!(send(&mut viewport, Input::BottomReached).is_empty());
    assert!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Newer { after: 175 },
                first: Some(176),
                last: Some(199),
                more: false,
            }
        )
        .is_empty()
    );
    assert!(send(&mut viewport, Input::BottomReached).is_empty());
    assert!(viewport.state().is_following());
}

#[test]
fn a_jump_loads_the_page_it_lands_on_then_scrolls_to_the_item() {
    let mut viewport = Viewport::default();
    send(
        &mut viewport,
        Input::Loaded {
            fetch: Fetch::Tail,
            first: Some(100),
            last: Some(199),
            more: true,
        },
    );
    send(&mut viewport, Input::Jump(Jump::To(75)));
    assert_eq!(
        send(
            &mut viewport,
            Input::Present {
                position: 75,
                loaded: false
            }
        ),
        [Effect::Fetch(Fetch::At { position: 75 })],
        "the page it is on, at once"
    );
    assert!(
        send(
            &mut viewport,
            Input::ScrolledAway {
                anchor: 150,
                through: 160
            }
        )
        .is_empty()
    );
    assert!(matches!(
        viewport.state(),
        State::Jumping { loading: true, .. }
    ));
    assert!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::At { position: 75 },
                first: Some(75),
                last: Some(174),
                more: true,
            }
        )
        .is_empty()
    );
    assert_eq!((viewport.oldest, viewport.gap), (Some(75), Some(174)));
    assert_eq!(
        send(
            &mut viewport,
            Input::Present {
                position: 75,
                loaded: true
            }
        ),
        [Effect::ScrollTo(75)]
    );
    assert_eq!(
        viewport.state(),
        &State::Reading {
            anchor: 75,
            through: 75
        }
    );
}

#[test]
fn latest_jump_reloads_an_evicted_tail_and_failure_returns_to_history() {
    let mut viewport = Viewport::default();
    send(
        &mut viewport,
        Input::Loaded {
            fetch: Fetch::Tail,
            first: Some(100),
            last: Some(199),
            more: true,
        },
    );
    send(&mut viewport, Input::EvictedNewer { last: 175 });
    assert_eq!(
        send(&mut viewport, Input::Jump(Jump::Latest)),
        [Effect::Fetch(Fetch::Tail)]
    );
    assert!(send(&mut viewport, Input::LoadFailed).is_empty());
    assert_eq!(
        viewport.state(),
        &State::Reading {
            anchor: 100,
            through: 100
        }
    );
    assert!(!viewport.in_flight);
    assert_eq!(
        send(&mut viewport, Input::Jump(Jump::Latest)),
        [Effect::Fetch(Fetch::Tail)]
    );
    assert_eq!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Tail,
                first: Some(200),
                last: Some(299),
                more: true,
            }
        ),
        [Effect::ScrollToEnd]
    );
    assert!(viewport.state().is_following());
    assert!(viewport.gap.is_none());
}

fn parked_following() -> Viewport {
    let mut viewport = Viewport::default();
    send(&mut viewport, Input::Open);
    send(
        &mut viewport,
        Input::Loaded {
            fetch: Fetch::Tail,
            first: Some(100),
            last: Some(199),
            more: true,
        },
    );
    viewport
}

#[test]
fn resuming_a_parked_thread_refreshes_its_tail_and_keeps_its_history() {
    let mut viewport = parked_following();
    viewport.in_flight = true;

    assert_eq!(
        send(&mut viewport, Input::Resume),
        [Effect::Fetch(Fetch::Refresh)]
    );
    assert_eq!(
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Refresh,
                first: Some(150),
                last: Some(210),
                more: true,
            }
        ),
        [Effect::ScrollToEnd]
    );
    assert_eq!(viewport.oldest, Some(100));
    assert!(!viewport.exhausted);
    assert!(!viewport.in_flight);
    assert!(viewport.state().is_following());
}

#[test]
fn resuming_a_window_with_a_gap_reads_it_without_refreshing() {
    let mut viewport = parked_following();
    send(&mut viewport, Input::Jump(Jump::To(40)));
    send(
        &mut viewport,
        Input::Present {
            position: 40,
            loaded: false,
        },
    );
    send(
        &mut viewport,
        Input::Loaded {
            fetch: Fetch::At { position: 40 },
            first: Some(40),
            last: Some(60),
            more: true,
        },
    );

    assert!(send(&mut viewport, Input::Resume).is_empty());
    assert!(!viewport.in_flight);
    assert_eq!(
        viewport.state(),
        &State::Reading {
            anchor: 40,
            through: 40
        }
    );
}
