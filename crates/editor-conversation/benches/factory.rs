use aqua_text::document::SelectionDocumentId;
use artisan_api::{Collection, ErrorKind, Item, ItemId, ItemKind, Phase, Status, ThreadId};
use divan::Bencher;
use editor_conversation::controller::Conversation;
use editor_conversation::fold::{PlainSection, Row, Slot};
use editor_conversation::presentation::{MarkdownOwners, PresentationScope};
use editor_conversation::view::factory::RowFactory;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;
use test_support::editor::rows::{realized, row_factory, row_model, services, source_ids};

const ERROR_PARTS: usize = 4_000;
const REPLY_PARAGRAPHS: usize = 400;
const ANSWER_BYTES: usize = 64 * 1024;

fn main() {
    divan::main();
}

fn item(id: &str, kind: ItemKind) -> Arc<Item> {
    Arc::new(Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    })
}

struct Rows {
    _stubbed: StubbedHost,
    _app: aqua::UiApp,
    factory: RowFactory,
    listed: Vec<Row>,
}

impl Rows {
    fn blocks_of(item: &Arc<Item>, slot: Slot, blocks: usize) -> Self {
        Self::of(
            item,
            (0..blocks)
                .map(|block| Row::Text {
                    id: item.id.clone(),
                    slot,
                    block,
                    run: None,
                })
                .collect(),
        )
    }

    fn of(item: &Arc<Item>, rows: Vec<Row>) -> Self {
        let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
        let mut conversation = Conversation::new(&stubbed.core);
        let host = stubbed.host.clone();
        conversation.open_thread(&mut stubbed.core, host, ThreadId("thread".into()));
        let scope = PresentationScope::capture(&conversation, &stubbed.core)
            .expect("bench: the thread opens on its own host");
        let realized_rows = rows
            .iter()
            .map(|row| realized(row_model(row.clone(), [Arc::clone(item)])))
            .collect();
        let (_, allocate) = source_ids(0);
        let mut app = test_support::editor::ui::ui_app(800.0, 600.0);
        let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(1))));
        markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
        let factory = row_factory(
            services(&stubbed.core, &scope),
            realized_rows,
            scope,
            markdown,
            true,
            allocate,
        );
        Self {
            _stubbed: stubbed,
            _app: app,
            factory,
            listed: rows,
        }
    }

    fn realize_all(&self) -> usize {
        self.listed
            .iter()
            .enumerate()
            .map_while(|(index, row)| self.factory.realize(index, &row.key()).ok())
            .count()
    }

    fn blocks(&self) -> usize {
        let realized = self.realize_all();
        assert!(realized > 1, "bench: the text spans several blocks");
        realized
    }
}

#[divan::bench(sample_count = 20)]
fn realize_every_block_of_a_long_inline_error(bencher: Bencher<'_, '_>) {
    let error = (0..ERROR_PARTS)
        .map(|part| format!("error[{part}]: the build broke"))
        .collect::<Vec<_>>()
        .join(" ");
    let failed = item(
        "error",
        ItemKind::Error {
            kind: ErrorKind::Execution,
            message: error.into(),
            retryable: false,
        },
    );
    let probe = Rows::blocks_of(&failed, Slot::Plain(PlainSection::Error), ERROR_PARTS);
    let rows = Rows::blocks_of(&failed, Slot::Plain(PlainSection::Error), probe.blocks());
    bencher.bench_local(|| rows.realize_all());
}

#[divan::bench(sample_count = 20)]
fn realize_every_block_of_a_long_reply(bencher: Bencher<'_, '_>) {
    let reply = (0..REPLY_PARAGRAPHS)
        .map(|paragraph| {
            format!(
                "Paragraph {paragraph} keeps **row {paragraph}** steady while `paint_{paragraph}` runs, so the reader never sees text move."
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let replied = item(
        "reply",
        ItemKind::AssistantMessage {
            text: reply.into(),
            phase: Phase::Final,
        },
    );
    let probe = Rows::blocks_of(&replied, Slot::Prose, REPLY_PARAGRAPHS);
    let rows = Rows::blocks_of(&replied, Slot::Prose, probe.blocks());
    bencher.bench_local(|| rows.realize_all());
}

#[divan::bench(sample_count = 20)]
fn realize_a_long_answer(bencher: Bencher<'_, '_>) {
    let answer = format!(
        "You answered: {}",
        "Use the atlas renderer and repaint only the damaged rows. ".repeat(ANSWER_BYTES / 58)
    );
    let answered = item(
        "answer",
        ItemKind::Question {
            blocking: true,
            questions: Collection::Inline(Vec::new()),
            answers: Some(Collection::Inline(Vec::new())),
            answer_text: Some(answer.into()),
        },
    );
    let rows = Rows::of(&answered, vec![Row::Prompt(answered.id.clone())]);
    assert_eq!(rows.realize_all(), 1, "bench: the answer realizes");
    bencher.bench_local(|| rows.realize_all());
}
