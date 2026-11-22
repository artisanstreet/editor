use crate::conversation::controller::Conversation;
use crate::conversation::presentation::{MarkdownOwners, PresentationScope};
use crate::conversation::view::factory::{RowFactory, RowServiceParts, RowServices, WorkResources};
use crate::conversation::view::motion::RowMotion;
use crate::conversation::view::reveal::{MountedReveals, RevealSync};
use crate::conversation::view::row_cache::RowCache;
use crate::conversation::view::status::StatusShimmers;
use crate::conversation::view::transcript::layout_batched_list;
use aqua::UiApp;
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use editor_core::{Core, CoreEvent};
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use test_support::editor::ui::{mount, painted_texts, ui_app};
use test_support::wait::{LIVENESS, wait_until};

pub(crate) struct Transcript {
    app: UiApp,
    scope: PresentationScope,
    markdown: Rc<RefCell<MarkdownOwners>>,
    reveals: Rc<RefCell<MountedReveals>>,
    rows: Rc<RefCell<RowCache>>,
    statuses: Rc<RefCell<StatusShimmers>>,
    motion: Rc<RefCell<RowMotion>>,
    next_source: Rc<Cell<u64>>,
}

impl Transcript {
    pub(crate) fn open(conversation: &Conversation, core: &Core) -> Self {
        let scope =
            PresentationScope::capture(conversation, core).expect("the open thread has a scope");
        let mut app = ui_app(900.0, 700.0);
        let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(1))));
        markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
        Self {
            app,
            scope,
            markdown,
            reveals: Rc::new(RefCell::new(MountedReveals::default())),
            rows: Rc::new(RefCell::new(RowCache::default())),
            statuses: Rc::new(RefCell::new(StatusShimmers::default())),
            motion: Rc::new(RefCell::new(RowMotion::default())),
            next_source: Rc::new(Cell::new(0)),
        }
    }

    pub(crate) fn render(&mut self, conversation: &mut Conversation, core: &Core) -> RevealSync {
        let services = RowServices::new(
            &self.scope,
            RowServiceParts {
                provider: core
                    .source_provider(&self.scope.host, self.scope.instance)
                    .expect("the stub host is connected"),
                document: SelectionDocumentId(1),
                on_source: Rc::new(|_| {}),
                stored: Rc::default(),
                images: core
                    .host_if_instance(&self.scope.host, self.scope.instance)
                    .expect("the stub host is connected")
                    .blob_images(),
                on_blob: Rc::new(|_, _, _| {}),
            },
        )
        .expect("the open thread's own host serves its rows");
        let next_source = Rc::clone(&self.next_source);
        let factory = Rc::new(
            RowFactory::new(
                Vec::new(),
                self.scope.clone(),
                Rc::clone(&self.markdown),
                Rc::clone(&self.statuses),
                ArtisanTheme::for_mode(ThemeMode::Dark),
                true,
                std::time::Instant::now(),
                Rc::new(move || {
                    next_source.set(next_source.get() + 1);
                    SelectionSourceId(next_source.get())
                }),
                Rc::new(|| {}),
                Rc::new(|_| {}),
                Rc::new(|_| {}),
                Rc::new(|_| {}),
                Rc::new(|_| {}),
                WorkResources {
                    motion: Rc::clone(&self.motion),
                    names: HashMap::new(),
                    seconds: 0.0,
                    timestamp: 0,
                },
                services,
            )
            .with_row_cache(Rc::clone(&self.rows))
            .with_lazy_rows(conversation, core, None)
            .expect("the open thread has rows")
            .with_reveals(conversation, core, Rc::clone(&self.reveals), false)
            .expect("the open thread is current")
            .with_media(Rc::new(|_, _| None)),
        );
        let revisions = Rc::clone(&factory);
        self.app = ui_app(900.0, 700.0);
        mount(
            &mut self.app,
            layout_batched_list(
                conversation,
                24.0,
                factory.layout_callback(),
                Rc::new(move |index, key| revisions.revision(index, key)),
                Rc::new(|_| {}),
                Rc::new(|_, _| 24.0),
            ),
        );
        self.app
            .build_frame()
            .expect("every row of the open thread mounts");
        factory.sync_reveals(&mut self.app, conversation, core)
    }

    pub(crate) fn texts(&self) -> Vec<String> {
        painted_texts(&self.app)
    }

    pub(crate) fn shows_selector(&self, selector: &str) -> bool {
        self.app.tree().traverse().into_iter().any(|node| {
            self.app
                .tree()
                .get(node)
                .is_some_and(|record| record.debug_selector.as_deref() == Some(selector))
        })
    }
}

pub(crate) fn deliver(
    core: &mut Core,
    conversation: &mut Conversation,
    what: &str,
    done: impl Fn(&CoreEvent) -> bool,
) {
    wait_until(what, LIVENESS, || {
        let mut delivered = false;
        core.drain_with(|core, event| {
            delivered |= done(&event);
            conversation.core_event(core, &event, false);
        });
        delivered
    });
}
