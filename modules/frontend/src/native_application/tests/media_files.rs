//! The media-file bridge against the real application event dispatch: a
//! block's fetch becomes one `ReadMediaFile`, and the Forge's answer, or the
//! service stopping, settles every waiter.

use crate::native_application::{NativeApplication, NativeTestCommandSink};
use crate::native_transport_service::{
    CommandSendError, NativeTransportCommand, NativeTransportEvent, ServiceStopStatus,
};
use artisan_domain::{MediaPathAnchor, ThreadId};
use artisan_ui::markdown_renderer::MediaRequest;
use gpui::TestAppContext;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};

fn sink(
    outcomes: impl IntoIterator<Item = Result<(), CommandSendError>>,
) -> (
    NativeTestCommandSink,
    Rc<RefCell<Vec<NativeTransportCommand>>>,
) {
    let commands = Rc::new(RefCell::new(Vec::new()));
    let sink = NativeTestCommandSink {
        commands: commands.clone(),
        outcomes: Rc::new(RefCell::new(outcomes.into_iter().collect::<VecDeque<_>>())),
    };
    (sink, commands)
}

fn request(path: &str, from_home: bool) -> MediaRequest {
    MediaRequest {
        scope: "thread-media".into(),
        path: path.to_owned(),
        from_home,
    }
}

#[gpui::test]
fn one_read_answers_every_waiter_for_the_same_file(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = sink([]);
    let thread = ThreadId::parse("thread-media").unwrap();
    let fetcher = cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            application.media_files.fetcher(&thread)
        })
    });
    let first = fetcher.fetch(request("docs/shot.png", false));
    let second = fetcher.fetch(request("docs/shot.png", false));
    let home = fetcher.fetch(request("Downloads/clip.mp3", true));
    cx.run_until_parked();

    let reads: Vec<_> = commands
        .borrow()
        .iter()
        .map(|command| match command {
            NativeTransportCommand::ReadMediaFile(read) => read.clone(),
            other => panic!("expected a media read, got {other:?}"),
        })
        .collect();
    assert_eq!(reads.len(), 2, "the same file is asked for once");
    assert_eq!(reads[0].thread_id(), &thread);
    assert_eq!(reads[0].path().text(), "docs/shot.png");
    assert_eq!(reads[0].path().anchor(), MediaPathAnchor::Project);
    assert_eq!(reads[1].path().anchor(), MediaPathAnchor::Home);

    let bytes: Arc<[u8]> = Arc::from(&b"\x89PNG"[..]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::MediaFileLoaded {
                    read: reads[0].clone(),
                    bytes: bytes.clone(),
                },
                cx,
            );
            application
                .handle_service_event(NativeTransportEvent::Stopped(ServiceStopStatus::Clean), cx);
        });
    });
    assert_eq!(futures::executor::block_on(first), Some(bytes.clone()));
    assert_eq!(futures::executor::block_on(second), Some(bytes));
    assert_eq!(
        futures::executor::block_on(home),
        None,
        "a stopped service answers what it never will"
    );
}

#[gpui::test]
fn a_read_that_cannot_be_sent_settles_empty(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _commands) = sink([Err(CommandSendError::Stopped)]);
    let fetcher = cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            application
                .media_files
                .fetcher(&ThreadId::parse("thread-media").unwrap())
        })
    });
    let fetch = fetcher.fetch(request("docs/shot.png", false));
    cx.run_until_parked();
    assert_eq!(futures::executor::block_on(fetch), None);
    // A path no Forge could take never leaves the Editor.
    let invalid = fetcher.fetch(request("", false));
    assert_eq!(futures::executor::block_on(invalid), None);
}
