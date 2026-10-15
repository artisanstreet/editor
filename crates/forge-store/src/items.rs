//! Items: engine output, accepted input, answers and transcript pages.

use crate::{
    ItemNotFound, NewItem, PendingInputs, Refused, SendRejected, Store, collection, content,
    fresh_item, read,
};
use anyhow::{Context, Result, ensure};
use artisan_api::{
    EngineId, Item, ItemId, ItemKind, ItemsPage, ItemsQuery, Run, RunId, RunStatus, SendMessage,
    Seq, Status, ThreadId,
};
use rusqlite::{OptionalExtension as _, params};

/// What accepting a message changes on its thread besides activity. The run
/// layer decides it; the store applies it in the input's own commit.
#[derive(Clone, Debug, Default)]
pub struct Acceptance {
    /// Becomes the thread's title when it has none.
    pub title: Option<String>,
    /// Saves the thread's resulting config as `default_engine_config`.
    pub remember_config: bool,
}

impl Store {
    /// Inserts the item or updates it by id. Position and creation time never
    /// change, and a terminal status never reopens.
    pub async fn upsert_item(&self, new: NewItem) -> Result<Item> {
        let (id, thread_id) = (new.id.clone(), new.thread_id.clone());
        self.write(move |tx| {
            let native_id = new.native_id.clone();
            let item = if let Some(old) = read::item(tx, &new.id)? {
                ensure!(
                    old.thread_id == new.thread_id,
                    "item belongs to thread {}",
                    old.thread_id
                );
                let mut kind = new.kind;
                // Engines don't know Artisan's name for a subagent, and an
                // agent adopted by a later process no longer knows its
                // prompt: keep them.
                if let (
                    ItemKind::Agent {
                        label,
                        prompt,
                        agent_id,
                        context_used,
                        model,
                        ..
                    },
                    ItemKind::Agent {
                        label: old_label,
                        prompt: old_prompt,
                        agent_id: old_agent_id,
                        context_used: old_context,
                        model: old_model,
                        ..
                    },
                ) = (&mut kind, &old.kind)
                {
                    for (slot, old) in [
                        (label, old_label),
                        (agent_id, old_agent_id),
                        (model, old_model),
                    ] {
                        if slot.is_none() {
                            slot.clone_from(old);
                        }
                    }
                    if prompt.is_none() {
                        prompt.clone_from(old_prompt);
                    }
                    if context_used.is_none() {
                        *context_used = *old_context;
                    }
                }
                let item = Item {
                    id: old.id.clone(),
                    thread_id: old.thread_id.clone(),
                    position: old.position,
                    seq: old.seq,
                    // An item stays in the run that made it: a resumed agent
                    // reports from a later run, but its row stays where it
                    // was launched, in its own turn.
                    run_id: old.run_id.clone().or(new.run_id),
                    parent_id: new.parent_id.or_else(|| old.parent_id.clone()),
                    status: old.next_status(new.status),
                    created_at: old.created_at,
                    updated_at: old.updated_at,
                    kind,
                };
                tx.save_item(item, Some(old))?
            } else {
                let created_at = tx.created_at();
                let mut new = new;
                // A new subagent gets a name no other in its thread has.
                if let ItemKind::Agent {
                    label: slot @ None, ..
                } = &mut new.kind
                {
                    let taken = read::agent_labels(tx, &new.thread_id)?;
                    *slot = Some(artisan_api::choose_agent_name(
                        new.id.as_str(),
                        taken.iter().map(String::as_str),
                    ));
                }
                tx.save_item(fresh_item(new, created_at), None)?
            };
            tx.set_native_id(&item.id, native_id.as_deref())?;
            Ok(item)
        })
        .await
        .with_context(|| format!("upsert item {id} in thread {thread_id}"))
    }

    /// A subagent an earlier run left stopped is working again in `run`
    /// (resumed by a message to it): it reopens there, so it reads as
    /// working and the run settles it. Statuses otherwise never leave a
    /// terminal state. Returns the stored agent.
    pub async fn reopen_agent(&self, id: &ItemId, run: &RunId) -> Result<Option<Item>> {
        let (id, run) = (id.clone(), run.clone());
        let what = id.clone();
        self.write(move |tx| {
            let Some(old) = read::item(tx, &id)? else {
                return Ok(None);
            };
            if !matches!(old.kind, ItemKind::Agent { .. }) || !old.status.is_terminal() {
                return Ok(Some(old));
            }
            let item = Item {
                run_id: old.run_id.clone().or(Some(run)),
                status: Status::Running,
                ..old.clone()
            };
            tx.save_item(item, Some(old)).map(Some)
        })
        .await
        .with_context(|| format!("reopen agent {what}"))
    }

    /// Inserts accepted input once and records its thread activity in the same commit.
    pub async fn insert_user_message(&self, new: NewItem) -> Result<(Item, bool)> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let (item, created) = tx.insert_input(new, &blob_dir)?;
            if created {
                tx.mark_activity(&item.thread_id, item.seq)?;
            }
            Ok((item, created))
        })
        .await
    }

    /// Accepts a message once. A replay changes nothing; the first accept
    /// applies its config and `acceptance` in the same commit.
    pub async fn send_message(
        &self,
        thread_id: ThreadId,
        message: SendMessage,
        steer: bool,
        acceptance: Acceptance,
    ) -> Result<(Item, bool, EngineId)> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let mut thread = read::thread(tx, &thread_id)?
                .ok_or_else(|| SendRejected::ThreadNotFound(thread_id.clone()))?;
            let engine = thread.config.engine;
            let (item, created) = tx.insert_input(
                NewItem {
                    id: message.id,
                    thread_id,
                    run_id: None,
                    parent_id: None,
                    native_id: None,
                    status: Status::Running,
                    kind: ItemKind::UserMessage {
                        text: message.text.into(),
                        images: message.images,
                        steer,
                    },
                },
                &blob_dir,
            )?;
            if created {
                if let Some(config) = message.config {
                    thread.config = config;
                }
                if thread.title.is_none() {
                    thread.title = acceptance.title;
                }
                tx.accept_command(thread, item.seq, acceptance.remember_config, message.draft)?;
            }
            Ok((item, created, engine))
        })
        .await
    }

    /// Accepts a thread setting command once, as a notice instead of model
    /// input. `apply` changes the thread's config and returns the notice; a
    /// replay is a notice with the same id that starts with `replay_prefix`.
    pub async fn configure_thread(
        &self,
        thread_id: ThreadId,
        message: SendMessage,
        replay_prefix: String,
        remember_config: bool,
        apply: impl FnOnce(&mut artisan_api::EngineConfig) -> String + Send + 'static,
    ) -> Result<Item> {
        self.write(move |tx| {
            let mut thread = read::thread(tx, &thread_id)?
                .ok_or_else(|| SendRejected::ThreadNotFound(thread_id.clone()))?;
            if let Some(existing) = read::item(tx, &message.id)? {
                let same = existing.thread_id == thread_id
                    && matches!(&existing.kind,
                        ItemKind::Notice { kind: artisan_api::NoticeKind::Info, text }
                        if text.inline().is_some_and(|text| text.starts_with(&replay_prefix)));
                ensure!(same, SendRejected::IdConflict(message.id));
                return Ok(existing);
            }
            if let Some(config) = message.config {
                thread.config = config;
            }
            let text = apply(&mut thread.config);
            let new = NewItem {
                id: message.id,
                thread_id,
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Info,
                    text: text.into(),
                },
            };
            let item = tx.save_item(fresh_item(new, tx.created_at()), None)?;
            tx.accept_command(thread, item.seq, remember_config, message.draft)?;
            Ok(item)
        })
        .await
    }

    /// Delivery acknowledgment is atomic for the entire initial batch or one steer.
    pub async fn accept_inputs(
        &self,
        thread: ThreadId,
        run: RunId,
        ids: Vec<ItemId>,
    ) -> Result<()> {
        self.write(move |tx| {
            for id in ids {
                if let Some(old) = read::item(tx, &id)? {
                    ensure!(
                        old.thread_id == thread && matches!(old.kind, ItemKind::UserMessage { .. }),
                        "input {id} does not belong to thread {thread}"
                    );
                    if old.status == Status::Running {
                        let item = Item {
                            run_id: Some(run.clone()),
                            status: Status::Done,
                            ..old.clone()
                        };
                        tx.save_item(item, Some(old))?;
                    }
                }
            }
            Ok(())
        })
        .await
    }

    /// Status changes read the durable kind, instead of retaining another full copy in a run.
    pub async fn set_item_status(&self, id: ItemId, status: Status) -> Result<Option<Item>> {
        self.write(move |tx| {
            let Some(old) = read::item(tx, &id)? else {
                return Ok(None);
            };
            let item = Item {
                status: old.next_status(status),
                ..old.clone()
            };
            tx.save_item(item, Some(old)).map(Some)
        })
        .await
    }

    pub async fn cancel_pending_inputs(&self, thread: ThreadId, through: Seq) -> Result<()> {
        self.write(move |tx| {
            let inputs: Vec<Item> = read::items(
                tx,
                "SELECT body FROM items WHERE thread_id=?1 AND position<=?2 AND status='running'
                    AND kind_type='user_message'",
                params![thread.as_str(), through],
            )?;
            let mut activity = None;
            for old in inputs {
                let input = Item {
                    status: Status::Cancelled,
                    ..old.clone()
                };
                activity = Some(tx.save_item(input, Some(old))?.seq);
            }
            if let Some(seq) = activity {
                tx.mark_activity(&thread, seq)?;
            }
            Ok(())
        })
        .await
    }

    /// All remaining outputs and the final run commit together; activity changes once.
    pub async fn settle_run(&self, run: Run, open: Vec<ItemId>) -> Result<Run> {
        self.write(move |tx| {
            let status = if run.status == RunStatus::Done {
                Status::Done
            } else {
                Status::Cancelled
            };
            for id in open {
                if let Some(old) = read::item(tx, &id)?
                    && !old.status.is_terminal()
                {
                    ensure!(
                        old.thread_id == run.thread_id,
                        "output belongs to another thread"
                    );
                    if run.status != RunStatus::Cancelled
                        && old.status == Status::AwaitingUser
                        && matches!(
                            old.kind,
                            ItemKind::Question {
                                blocking: false,
                                ..
                            }
                        )
                    {
                        continue;
                    }
                    // A process exit closes any unanswered blocking interaction;
                    // completion of the run does not imply a callback was answered.
                    let item = Item {
                        status: if old.status == Status::AwaitingUser {
                            Status::Cancelled
                        } else {
                            status
                        },
                        ..old.clone()
                    };
                    tx.save_item(item, Some(old))?;
                }
            }
            let run = tx.save_run(run)?;
            tx.mark_activity(&run.thread_id, run.seq)?;
            Ok(run)
        })
        .await
    }

    /// Answers an asynchronous question after its process exits, as a durable next-turn input.
    pub async fn answer_question(
        &self,
        id: ItemId,
        answers: Vec<Option<artisan_api::QuestionAnswer>>,
    ) -> Result<Item> {
        let blob_dir = self.inner.blob_dir.clone();
        self.write(move |tx| {
            let old = read::item(tx, &id)?.ok_or_else(|| ItemNotFound(id.clone()))?;
            let mut item = old.clone();
            let ItemKind::Question {
                blocking: false,
                questions,
                answers: saved,
                ..
            } = &mut item.kind
            else {
                return Err(
                    Refused("This question needs its engine, which has stopped.".into()).into(),
                );
            };
            collection::validate_answers(tx, questions, &answers)?;
            let input_id = ItemId(format!("answer-{id}"));
            if item.status.is_terminal() {
                ensure!(
                    collection::answers_equal(tx, saved, &answers)?,
                    Refused("This question was already answered differently.".into())
                );
                return read::item(tx, &input_id)?.context("question answer input is missing");
            }
            ensure!(
                item.status == Status::AwaitingUser,
                Refused("This question is not waiting for an answer.".into())
            );
            let mut text = String::from("Answers to the agent's questions:\n");
            collection::visit_questions(tx, questions, |index, question| {
                text.push_str(&content::resolve(tx, &question.text)?);
                text.push_str("\nAnswer: ");
                match &answers[index] {
                    Some(answer) => text.push_str(&collection::answer_text(tx, question, answer)?),
                    None => text.push_str("Skipped"),
                }
                text.push_str("\n\n");
                Ok(())
            })?;
            *saved = Some(answers.into());
            item.status = Status::Done;
            tx.save_item(item.clone(), Some(old))?;
            let (input, _) = tx.insert_input(
                NewItem {
                    id: input_id,
                    thread_id: item.thread_id,
                    run_id: None,
                    parent_id: None,
                    native_id: None,
                    status: Status::Running,
                    kind: ItemKind::UserMessage {
                        text: text.into(),
                        images: Vec::new(),
                        steer: false,
                    },
                },
                &blob_dir,
            )?;
            tx.mark_activity(&input.thread_id, input.seq)?;
            Ok(input)
        })
        .await
    }

    /// Validate selected opaque IDs through the immutable record-key index.
    /// A native callback never needs to load every question option.
    pub async fn validate_question_answers(
        &self,
        id: &ItemId,
        answers: &[Option<artisan_api::QuestionAnswer>],
    ) -> Result<()> {
        let id = id.clone();
        let answers = answers.to_vec();
        self.read(move |conn| {
            let item = read::item(conn, &id)?.ok_or_else(|| ItemNotFound(id.clone()))?;
            let ItemKind::Question { questions, .. } = item.kind else {
                return Err(Refused(format!("Item {id} is not a question.")).into());
            };
            collection::validate_answers(conn, &questions, &answers)
        })
        .await
    }

    /// Native callbacks use complete admitted canonical records. Rendering and
    /// state/bootstrap endpoints must use collection_range instead.
    pub async fn resolve_item_collections(&self, mut item: Item) -> Result<Item> {
        self.read(move |conn| {
            collection::hydrate_item(conn, &mut item.kind)?;
            Ok(item)
        })
        .await
    }

    pub async fn item(&self, id: &ItemId) -> Result<Option<Item>> {
        let id = id.clone();
        self.read(move |conn| read::item(conn, &id).with_context(|| format!("read item {id}")))
            .await
    }

    /// The engine's own id for an item: what an answer to it is routed by.
    pub async fn native_id(&self, id: &ItemId) -> Result<Option<String>> {
        let id = id.clone();
        self.read(move |conn| {
            conn.prepare_cached("SELECT native_id FROM items WHERE id=?1")?
                .query_row([id.as_str()], |row| row.get::<_, Option<String>>(0))
                .optional()
                .map(Option::flatten)
                .with_context(|| format!("read the engine id of item {id}"))
        })
        .await
    }

    /// The `Agent` item of `thread_id` an engine addresses as `address`.
    pub async fn agent_by_address(
        &self,
        thread_id: &ThreadId,
        address: &str,
    ) -> Result<Option<ItemId>> {
        let (thread_id, address) = (thread_id.clone(), address.to_owned());
        self.read(move |conn| read::agent_by_address(conn, &thread_id, &address))
            .await
    }

    /// The item an engine knows as `native_id`, within `run` when given.
    pub async fn item_by_native(
        &self,
        thread_id: &ThreadId,
        run: Option<&RunId>,
        native_id: &str,
    ) -> Result<Option<Item>> {
        let (thread_id, run, native_id) = (thread_id.clone(), run.cloned(), native_id.to_owned());
        self.read(move |conn| {
            read::item_by_native(conn, &thread_id, run.as_ref(), &native_id)
                .with_context(|| format!("read item {native_id:?} of thread {thread_id}"))
        })
        .await
    }

    /// Items by position. Without `after`, the page is the newest items
    /// before `before` (or the thread's tail); `more` says items exist past
    /// the far end of the page.
    pub async fn items_page(&self, thread_id: &ThreadId, query: ItemsQuery) -> Result<ItemsPage> {
        let thread_id = thread_id.clone();
        self.read(move |conn| {
            read::items_page(conn, &thread_id, &query)
                .with_context(|| format!("page items of thread {thread_id}"))
        })
        .await
    }

    /// A bounded root transcript or one agent's direct children, never the
    /// other agent transcripts that happen to share this thread's positions.
    pub async fn transcript_page(
        &self,
        thread_id: &ThreadId,
        parent: Option<&ItemId>,
        query: ItemsQuery,
    ) -> Result<ItemsPage> {
        let thread = thread_id.clone();
        let parent = parent.cloned();
        self.read(move |conn| read::scoped_items_page(conn, &thread, &query, Some(parent.as_ref())))
            .await
    }

    /// Approvals and questions waiting on the user, across threads.
    pub async fn items_awaiting_user(&self) -> Result<Vec<artisan_api::PendingItem>> {
        self.read(|conn| read::items_awaiting_user(conn).context("read items awaiting the user"))
            .await
    }

    /// At most `limit` eligible threads; the pending Item index is the queue.
    pub async fn pending_threads(
        &self,
        limit: usize,
        excluded: Vec<ThreadId>,
    ) -> Result<Vec<ThreadId>> {
        let limit = limit.min(8);
        self.read(move |conn| read::pending_threads(conn, limit, &excluded))
            .await
    }

    /// Reads a bounded initial batch and all observed pending input's sequence boundary in one snapshot.
    pub async fn pending_inputs(&self, thread_id: &ThreadId) -> Result<PendingInputs> {
        let thread = thread_id.clone();
        let blob_dir = self.inner.blob_dir.clone();
        self.read(move |conn| read::pending_inputs(conn, &thread, &blob_dir))
            .await
    }
}
