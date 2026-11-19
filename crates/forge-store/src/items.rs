use crate::content::ContentError;
use crate::error::AtStep as _;
use crate::error::{StorageError, operation_error};
use crate::writer::{InputFailure, Tx, WriteFailure, valid_config};
use crate::{
    AnswerRefused, CreateThreadError, NewItem, PendingInputs, SendRejected, Store, collection,
    content, fresh_item, read,
};
use artisan_api::{
    CreateThread, EngineId, Item, ItemId, ItemKind, ItemsPage, ItemsQuery, Run, RunId, RunStatus,
    SendMessage, Seq, Status, Thread, ThreadId,
};
use rusqlite::{OptionalExtension as _, params};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Acceptance {
    pub title: Option<String>,
    pub remember_config: bool,
    pub from_agent: Option<ItemId>,
    pub sender: Option<artisan_api::Sender>,
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnFailure {
    #[error(transparent)]
    Thread(#[from] CreateThreadError),
    #[error(transparent)]
    Input(#[from] InputFailure),
}

impl From<StorageError> for SpawnFailure {
    fn from(error: StorageError) -> Self {
        Self::Input(error.into())
    }
}

impl From<WriteFailure> for SpawnFailure {
    fn from(error: WriteFailure) -> Self {
        Self::Input(error.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerFailure {
    #[error("the item does not exist")]
    NotFound,
    #[error(transparent)]
    Refused(AnswerRefused),
    #[error(transparent)]
    Content(ContentError),
    #[error(transparent)]
    Input(#[from] InputFailure),
}

impl From<ContentError> for AnswerFailure {
    fn from(error: ContentError) -> Self {
        if let ContentError::Refused(refused) = error {
            Self::Refused(refused)
        } else {
            Self::Content(error)
        }
    }
}

impl From<AnswerRefused> for AnswerFailure {
    fn from(refused: AnswerRefused) -> Self {
        Self::Refused(refused)
    }
}

impl From<StorageError> for AnswerFailure {
    fn from(error: StorageError) -> Self {
        Self::Content(error.into())
    }
}

impl From<WriteFailure> for AnswerFailure {
    fn from(error: WriteFailure) -> Self {
        if let WriteFailure::Content(content) = error {
            content.into()
        } else {
            Self::Input(error.into())
        }
    }
}

operation_error!(UpsertItemError(WriteFailure), "save item {item} of thread {thread}", item: ItemId, thread: ThreadId);
operation_error!(ReopenAgentError(WriteFailure), "reopen agent {item} in run {run}", item: ItemId, run: RunId);
operation_error!(InsertMessageError(InputFailure), "insert message {item} into thread {thread}", item: ItemId, thread: ThreadId);
operation_error!(SendMessageError(InputFailure), "send message {item} to thread {thread}", item: ItemId, thread: ThreadId);
operation_error!(SettleAgentError(InputFailure), "report an agent's end to thread {thread}", thread: ThreadId);
operation_error!(SpawnAgentError(SpawnFailure), "spawn agent {agent} of thread {thread}", agent: ItemId, thread: ThreadId);
operation_error!(AcceptInputsError(WriteFailure), "accept the inputs of thread {thread} into run {run}", thread: ThreadId, run: RunId);
operation_error!(SetItemStatusError(WriteFailure), "set the status of item {item}", item: ItemId);
operation_error!(RequestContinueError(StorageError), "ask thread {thread} to continue", thread: ThreadId);
operation_error!(ReadContinueError(StorageError), "read the continue request of thread {thread}", thread: ThreadId);
operation_error!(BeginRunError(WriteFailure), "begin run {run} of thread {thread}", run: RunId, thread: ThreadId);
operation_error!(TakeContinueError(StorageError), "take the continue request of thread {thread}", thread: ThreadId);
operation_error!(UnparkError(StorageError), "unpark thread {thread}", thread: ThreadId);
operation_error!(ContinuationChainError(StorageError), "count the continuations of thread {thread}", thread: ThreadId);
operation_error!(
    NextParkedError(StorageError),
    "read when the next parked turn is due"
);
operation_error!(ContinuePendingError(StorageError), "read whether thread {thread} waits to continue", thread: ThreadId);
operation_error!(TurnUnsandboxedError(StorageError), "read whether the current turn of thread {thread} said it isn't sandboxed", thread: ThreadId);
operation_error!(CancelledItemsError(StorageError), "read the items run {run} left open", run: RunId);
operation_error!(RestoreInputsError(WriteFailure), "restore the undelivered inputs of thread {thread}", thread: ThreadId);
operation_error!(CancelInputsError(WriteFailure), "cancel the inputs of thread {thread} through seq {through}", thread: ThreadId, through: Seq);
operation_error!(SettleRunError(WriteFailure), "settle run {run} of thread {thread}", run: RunId, thread: ThreadId);
operation_error!(AnswerQuestionError(AnswerFailure), "answer question {item}", item: ItemId);
operation_error!(ValidateAnswersError(AnswerFailure), "check the answers to question {item}", item: ItemId);
operation_error!(ResolveCollectionsError(ContentError), "resolve the collections of item {item}", item: ItemId);
operation_error!(ReadItemError(StorageError), "read item {item}", item: ItemId);
operation_error!(NativeIdError(StorageError), "read the engine id of item {item}", item: ItemId);
operation_error!(AgentByAddressError(StorageError), "find the agent thread {thread}'s engine calls {address}", thread: ThreadId, address: String);
operation_error!(ItemByNativeError(StorageError), "find item {native:?} of thread {thread}", thread: ThreadId, native: String);
operation_error!(ItemsPageError(StorageError), "page the items of thread {thread}", thread: ThreadId);
operation_error!(TranscriptPageError(StorageError), "page the transcript of thread {thread}", thread: ThreadId);
operation_error!(TranscriptRowsError(StorageError), "page the transcript rows of thread {thread}", thread: ThreadId);
operation_error!(WorkPageError(StorageError), "page the work of thread {thread}", thread: ThreadId);
operation_error!(
    AwaitingItemsError(StorageError),
    "read the items waiting on the user"
);
operation_error!(
    PendingThreadsError(StorageError),
    "read the threads with pending input"
);
operation_error!(RunnableInputError(StorageError), "read whether thread {thread} has runnable input", thread: ThreadId);
operation_error!(PendingInputsError(StorageError), "read the pending inputs of thread {thread}", thread: ThreadId);

impl Store {
    pub async fn upsert_item(&self, new: NewItem) -> Result<Item, UpsertItemError> {
        let (item, thread) = (new.id.clone(), new.thread_id.clone());
        let bytes = crate::encoded::len(&new.kind);
        self.write_carrying(bytes, move |tx| tx.upsert_item(new))
            .await
            .map_err(|cause| UpsertItemError {
                item,
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn reopen_agent(
        &self,
        id: &ItemId,
        run: &RunId,
    ) -> Result<Option<Item>, ReopenAgentError> {
        let (item, reopened_in) = (id.clone(), run.clone());
        let (id, run) = (id.clone(), run.clone());
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
        .map_err(|cause| ReopenAgentError {
            item,
            run: reopened_in,
            cause: Box::new(cause),
        })
    }

    pub async fn insert_user_message(
        &self,
        new: NewItem,
    ) -> Result<(Item, bool), InsertMessageError> {
        let blob_dir = self.inner.blob_dir.clone();
        let (item, thread) = (new.id.clone(), new.thread_id.clone());
        let bytes = crate::encoded::len(&new.kind);
        self.write_acknowledged_carrying(bytes, move |tx| {
            let (item, created) = tx.insert_input(new, &blob_dir)?;
            if created {
                tx.mark_activity(&item.thread_id, item.seq)?;
            }
            Ok((item, created))
        })
        .await
        .map_err(|cause| InsertMessageError {
            item,
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn send_message(
        &self,
        thread_id: ThreadId,
        message: SendMessage,
        acceptance: Acceptance,
    ) -> Result<(Item, bool, EngineId), SendMessageError> {
        let blob_dir = self.inner.blob_dir.clone();
        let (item, thread) = (message.id.clone(), thread_id.clone());
        let bytes = crate::encoded::len(&message);
        self.write_acknowledged_carrying(bytes, move |tx| {
            tx.send_message(thread_id, message, acceptance, &blob_dir)
        })
        .await
        .map_err(|cause| SendMessageError {
            item,
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn settle_agent(
        &self,
        agent: Option<NewItem>,
        report: crate::AgentReport,
    ) -> Result<(Item, EngineId), SettleAgentError> {
        let blob_dir = self.inner.blob_dir.clone();
        let parent = report.thread.clone();
        self.write(move |tx| {
            if let Some(agent) = agent {
                tx.upsert_item(agent)?;
            }
            let crate::AgentReport {
                thread,
                message,
                acceptance,
            } = report;
            let (item, _, engine) = tx.send_message(thread, message, acceptance, &blob_dir)?;
            Ok((item, engine))
        })
        .await
        .map_err(|cause| SettleAgentError {
            thread: parent,
            cause: Box::new(cause),
        })
    }

    pub async fn spawn_agent(
        &self,
        agent: NewItem,
        child: CreateThread,
        prompt: SendMessage,
        acceptance: Acceptance,
    ) -> Result<(Item, Thread, Item), SpawnAgentError> {
        let blob_dir = self.inner.blob_dir.clone();
        let (agent_id, parent) = (agent.id.clone(), agent.thread_id.clone());
        self.write_acknowledged(move |tx| {
            let agent = tx.upsert_item(agent)?;
            let child = tx.create_thread(child)?;
            let (prompt, _, _) =
                tx.send_message(child.id.clone(), prompt, acceptance, &blob_dir)?;
            Ok((agent, child, prompt))
        })
        .await
        .map_err(|cause| SpawnAgentError {
            agent: agent_id,
            thread: parent,
            cause: Box::new(cause),
        })
    }

    pub async fn accept_inputs(
        &self,
        thread: ThreadId,
        run: RunId,
        ids: Vec<ItemId>,
        delivered: crate::Delivered,
    ) -> Result<(), AcceptInputsError> {
        let (accepting, into) = (thread.clone(), run.clone());
        self.write(move |tx| {
            for id in ids {
                if let Some(old) = read::item(tx, &id)? {
                    if old.thread_id != thread {
                        return Err(WriteFailure::ItemMoved {
                            item: id,
                            stored: old.thread_id,
                            requested: thread,
                        });
                    }
                    if !matches!(old.kind, ItemKind::UserMessage { .. }) {
                        return Err(WriteFailure::NotAnInput { item: id });
                    }
                    if matches!(old.status, Status::Running | Status::Cancelled) {
                        let mut kind = old.kind.clone();
                        if let ItemKind::UserMessage { steer, .. } = &mut kind {
                            *steer = delivered == crate::Delivered::Steer;
                        }
                        let item = Item {
                            run_id: Some(run.clone()),
                            status: Status::Done,
                            kind,
                            ..old.clone()
                        };
                        tx.save_item(item, Some(old))?;
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|cause| AcceptInputsError {
            thread: accepting,
            run: into,
            cause: Box::new(cause),
        })
    }

    pub async fn set_item_status(
        &self,
        id: ItemId,
        status: Status,
    ) -> Result<Option<Item>, SetItemStatusError> {
        let item = id.clone();
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
        .map_err(|cause| SetItemStatusError {
            item,
            cause: Box::new(cause),
        })
    }

    pub async fn request_continue(&self, thread: ThreadId) -> Result<(), RequestContinueError> {
        let asked = thread.clone();
        self.write_acknowledged(move |tx| {
            tx.request_continue(&thread, artisan_api::Continuation::Requested)
        })
        .await
        .map_err(|cause| RequestContinueError {
            thread: asked,
            cause: Box::new(cause),
        })
    }

    pub async fn continue_request(
        &self,
        thread: &ThreadId,
    ) -> Result<Option<artisan_api::Continuation>, ReadContinueError> {
        let asked = thread.clone();
        let thread = thread.clone();
        self.read(move |conn| read::continue_request(conn, &thread))
            .await
            .map_err(|cause| ReadContinueError {
                thread: asked,
                cause: Box::new(cause),
            })
    }

    pub async fn begin_run(&self, run: Run) -> Result<Run, BeginRunError> {
        let (id, thread) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| {
            tx.take_continue(&run.thread_id)?;
            tx.upsert_run(run)
        })
        .await
        .map_err(|cause| BeginRunError {
            run: id,
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn take_continue(
        &self,
        thread: ThreadId,
    ) -> Result<Option<artisan_api::Continuation>, TakeContinueError> {
        let taken = thread.clone();
        self.write(move |tx| tx.take_continue(&thread))
            .await
            .map_err(|cause| TakeContinueError {
                thread: taken,
                cause: Box::new(cause),
            })
    }

    pub async fn unpark(&self, thread: ThreadId) -> Result<bool, UnparkError> {
        let unparked = thread.clone();
        self.write_acknowledged(move |tx| tx.unpark(&thread))
            .await
            .map_err(|cause| UnparkError {
                thread: unparked,
                cause: Box::new(cause),
            })
    }

    pub async fn continuation_chain(
        &self,
        thread: ThreadId,
        continuation: artisan_api::Continuation,
        limit: usize,
    ) -> Result<usize, ContinuationChainError> {
        let counted = thread.clone();
        self.write(move |tx| tx.continuation_chain(&thread, continuation, limit))
            .await
            .map_err(|cause| ContinuationChainError {
                thread: counted,
                cause: Box::new(cause),
            })
    }

    pub async fn next_parked(&self) -> Result<Option<artisan_api::Timestamp>, NextParkedError> {
        self.read(|conn| {
            conn.query_row(include_str!("sql/continues/next_due.sql"), [], |row| {
                row.get(0)
            })
            .at("continues/next_due.sql")
        })
        .await
        .map_err(|cause| NextParkedError {
            cause: Box::new(cause),
        })
    }

    pub async fn continue_pending(&self, thread: &ThreadId) -> Result<bool, ContinuePendingError> {
        let asked = thread.clone();
        let thread = thread.clone();
        self.read(move |conn| {
            conn.query_row(
                include_str!("sql/continues/pending.sql"),
                [thread.as_str()],
                |row| row.get(0),
            )
            .at("continues/pending.sql")
        })
        .await
        .map_err(|cause| ContinuePendingError {
            thread: asked,
            cause: Box::new(cause),
        })
    }

    pub async fn turn_said_unsandboxed(
        &self,
        thread: &ThreadId,
    ) -> Result<bool, TurnUnsandboxedError> {
        let asked = thread.clone();
        let thread = thread.clone();
        self.read(move |conn| {
            conn.prepare_cached(include_str!("sql/items/turn_unsandboxed.sql"))
                .at("items/turn_unsandboxed.sql")?
                .query_row([thread.as_str()], |row| row.get(0))
                .at("items/turn_unsandboxed.sql")
        })
        .await
        .map_err(|cause| TurnUnsandboxedError {
            thread: asked,
            cause: Box::new(cause),
        })
    }

    pub async fn cancelled_items(&self, run: &RunId) -> Result<Vec<Item>, CancelledItemsError> {
        let stopped = run.clone();
        let run = run.clone();
        self.read(move |conn| {
            read::items(
                conn,
                "items/cancelled.sql",
                include_str!("sql/items/cancelled.sql"),
                [run.as_str()],
            )
        })
        .await
        .map_err(|cause| CancelledItemsError {
            run: stopped,
            cause: Box::new(cause),
        })
    }

    pub async fn restore_undelivered_inputs(
        &self,
        thread: ThreadId,
    ) -> Result<usize, RestoreInputsError> {
        let restored = thread.clone();
        self.write_acknowledged(move |tx| {
            let inputs: Vec<Item> = read::items(
                tx,
                "pending/undelivered_inputs.sql",
                include_str!("sql/pending/undelivered_inputs.sql"),
                params![thread.as_str()],
            )?;
            let mut activity = None;
            for old in &inputs {
                let input = Item {
                    status: Status::Running,
                    ..old.clone()
                };
                activity = Some(tx.save_item(input, Some(old.clone()))?.seq);
            }
            if let Some(seq) = activity {
                tx.mark_activity(&thread, seq)?;
            }
            Ok(inputs.len())
        })
        .await
        .map_err(|cause| RestoreInputsError {
            thread: restored,
            cause: Box::new(cause),
        })
    }

    pub async fn cancel_pending_inputs(
        &self,
        thread: ThreadId,
        through: Seq,
    ) -> Result<(), CancelInputsError> {
        let cancelled = thread.clone();
        self.write_acknowledged(move |tx| {
            tx.take_continue(&thread)?;
            let inputs: Vec<Item> = read::items(
                tx,
                "items/pending_inputs_through.sql",
                include_str!("sql/items/pending_inputs_through.sql"),
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
        .map_err(|cause| CancelInputsError {
            thread: cancelled,
            through,
            cause: Box::new(cause),
        })
    }

    pub async fn settle_run(
        &self,
        settlement: crate::Settlement,
    ) -> Result<(Run, bool), SettleRunError> {
        let crate::Settlement {
            run,
            open,
            attempted,
            park,
            note,
        } = settlement;
        let (id, thread) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| {
            if let Some(note) = note {
                tx.upsert_item(note)?;
            }
            let status = if run.status() == RunStatus::Done {
                Status::Done
            } else {
                Status::Cancelled
            };
            for id in open {
                if let Some(old) = read::item(tx, &id)?
                    && !old.status.is_terminal()
                {
                    if old.thread_id != run.thread_id {
                        return Err(WriteFailure::ItemMoved {
                            item: id,
                            stored: old.thread_id,
                            requested: run.thread_id,
                        });
                    }
                    if run.status() != RunStatus::Cancelled
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
            let parked = match park.filter(|_| run.status() == RunStatus::Failed) {
                Some(park) => tx.park(&run, park)?,
                None => false,
            };
            let undelivered = match run.status() {
                RunStatus::Failed | RunStatus::Refused if !parked => Some(Status::Failed),
                RunStatus::Cancelled => Some(Status::Cancelled),
                RunStatus::Failed
                | RunStatus::Refused
                | RunStatus::Running
                | RunStatus::Done
                | RunStatus::Interrupted => None,
            };
            if let Some(undelivered) = undelivered {
                for id in attempted {
                    if let Some(old) = read::item(tx, &id)?
                        && old.status == Status::Running
                        && matches!(old.kind, ItemKind::UserMessage { .. })
                    {
                        let input = Item {
                            status: undelivered,
                            ..old.clone()
                        };
                        tx.save_item(input, Some(old))?;
                    }
                }
            }
            let run = tx.save_run(run)?;
            if run.status() == RunStatus::Interrupted {
                tx.continue_after_restart(&run)?;
            }
            tx.mark_activity(&run.thread_id, run.seq)?;
            Ok((run, parked))
        })
        .await
        .map_err(|cause| SettleRunError {
            run: id,
            thread,
            cause: Box::new(cause),
        })
    }

    pub async fn answer_question(
        &self,
        id: ItemId,
        answers: Vec<Option<artisan_api::QuestionAnswer>>,
    ) -> Result<Item, AnswerQuestionError> {
        let blob_dir = self.inner.blob_dir.clone();
        let item = id.clone();
        self.write_acknowledged(move |tx| answer(tx, &id, answers, &blob_dir))
            .await
            .map_err(|cause| AnswerQuestionError {
                item,
                cause: Box::new(cause),
            })
    }

    pub async fn validate_question_answers(
        &self,
        id: &ItemId,
        answers: &[Option<artisan_api::QuestionAnswer>],
    ) -> Result<(), ValidateAnswersError> {
        let question = id.clone();
        let id = id.clone();
        let answers = answers.to_vec();
        self.read(move |conn| {
            let item = read::item(conn, &id)?.ok_or(AnswerFailure::NotFound)?;
            let ItemKind::Question { questions, .. } = item.kind else {
                return Err(AnswerRefused::NotAQuestion { item: id }.into());
            };
            Ok(collection::validate_answers(conn, &questions, &answers)?)
        })
        .await
        .map_err(|cause| ValidateAnswersError {
            item: question,
            cause: Box::new(cause),
        })
    }

    pub async fn resolve_item_collections(
        &self,
        mut item: Item,
    ) -> Result<Item, ResolveCollectionsError> {
        let id = item.id.clone();
        self.read(move |conn| {
            collection::hydrate_item(conn, &mut item.kind)?;
            Ok(item)
        })
        .await
        .map_err(|cause| ResolveCollectionsError {
            item: id,
            cause: Box::new(cause),
        })
    }

    pub async fn item(&self, id: &ItemId) -> Result<Option<Item>, ReadItemError> {
        let item = id.clone();
        let id = id.clone();
        self.read(move |conn| read::item(conn, &id))
            .await
            .map_err(|cause| ReadItemError {
                item,
                cause: Box::new(cause),
            })
    }

    pub async fn native_id(&self, id: &ItemId) -> Result<Option<String>, NativeIdError> {
        let item = id.clone();
        let id = id.clone();
        self.read(move |conn| {
            conn.prepare_cached(include_str!("sql/items/native_id.sql"))
                .at("items/native_id.sql")?
                .query_row([id.as_str()], |row| row.get::<_, Option<String>>(0))
                .optional()
                .map(Option::flatten)
                .at("items/native_id.sql")
        })
        .await
        .map_err(|cause| NativeIdError {
            item,
            cause: Box::new(cause),
        })
    }

    pub async fn agent_by_address(
        &self,
        thread_id: &ThreadId,
        address: &str,
    ) -> Result<Option<ItemId>, AgentByAddressError> {
        let (thread, called) = (thread_id.clone(), address.to_owned());
        let (thread_id, address) = (thread_id.clone(), address.to_owned());
        self.read(move |conn| read::agent_by_address(conn, &thread_id, &address))
            .await
            .map_err(|cause| AgentByAddressError {
                thread,
                address: called,
                cause: Box::new(cause),
            })
    }

    pub async fn item_by_native(
        &self,
        thread_id: &ThreadId,
        run: Option<&RunId>,
        native_id: &str,
    ) -> Result<Option<Item>, ItemByNativeError> {
        let (thread, native) = (thread_id.clone(), native_id.to_owned());
        let (thread_id, run, native_id) = (thread_id.clone(), run.cloned(), native_id.to_owned());
        self.read(move |conn| read::item_by_native(conn, &thread_id, run.as_ref(), &native_id))
            .await
            .map_err(|cause| ItemByNativeError {
                thread,
                native,
                cause: Box::new(cause),
            })
    }

    pub async fn items_page(
        &self,
        thread_id: &ThreadId,
        query: ItemsQuery,
    ) -> Result<ItemsPage, ItemsPageError> {
        let thread = thread_id.clone();
        let thread_id = thread_id.clone();
        self.read(move |conn| read::items_page(conn, &thread_id, &query))
            .await
            .map_err(|cause| ItemsPageError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn transcript_page(
        &self,
        thread_id: &ThreadId,
        parent: Option<&ItemId>,
        query: ItemsQuery,
    ) -> Result<ItemsPage, TranscriptPageError> {
        let thread = thread_id.clone();
        let tail = crate::hot::Tail::items(parent, &query);
        let parent = parent.cloned();
        self.tail_page(thread_id, tail, move |conn| {
            read::scoped_items_page(conn, &thread, &query, Some(parent.as_ref()))
        })
        .await
        .map_err(|cause| TranscriptPageError {
            thread: thread_id.clone(),
            cause: Box::new(cause),
        })
    }

    pub async fn transcript_rows(
        &self,
        thread_id: &ThreadId,
        query: ItemsQuery,
    ) -> Result<ItemsPage, TranscriptRowsError> {
        let thread = thread_id.clone();
        let tail = crate::hot::Tail::rows(&query);
        self.tail_page(thread_id, tail, move |conn| {
            read::transcript_page(conn, &thread, &query)
        })
        .await
        .map_err(|cause| TranscriptRowsError {
            thread: thread_id.clone(),
            cause: Box::new(cause),
        })
    }

    pub async fn work_page(
        &self,
        thread_id: &ThreadId,
        query: ItemsQuery,
    ) -> Result<ItemsPage, WorkPageError> {
        let thread = thread_id.clone();
        let thread_id = thread_id.clone();
        self.read(move |conn| read::work_page(conn, &thread_id, &query))
            .await
            .map_err(|cause| WorkPageError {
                thread,
                cause: Box::new(cause),
            })
    }

    pub async fn items_awaiting_user(
        &self,
    ) -> Result<Vec<artisan_api::PendingItem>, AwaitingItemsError> {
        self.read(read::items_awaiting_user)
            .await
            .map_err(|cause| AwaitingItemsError {
                cause: Box::new(cause),
            })
    }

    pub async fn pending_threads(
        &self,
        limit: usize,
        excluded: Vec<ThreadId>,
    ) -> Result<Vec<ThreadId>, PendingThreadsError> {
        let limit = limit.min(8);
        let now = self.now();
        self.read(move |conn| read::pending_threads(conn, limit, &excluded, now))
            .await
            .map_err(|cause| PendingThreadsError {
                cause: Box::new(cause),
            })
    }

    pub async fn has_runnable_input(
        &self,
        thread_id: &ThreadId,
    ) -> Result<bool, RunnableInputError> {
        let asked = thread_id.clone();
        let thread = thread_id.clone();
        self.read(move |conn| read::has_runnable_input(conn, &thread))
            .await
            .map_err(|cause| RunnableInputError {
                thread: asked,
                cause: Box::new(cause),
            })
    }

    pub async fn pending_inputs(
        &self,
        thread_id: &ThreadId,
    ) -> Result<PendingInputs, PendingInputsError> {
        let asked = thread_id.clone();
        let thread = thread_id.clone();
        let blob_dir = self.inner.blob_dir.clone();
        self.read(move |conn| read::pending_inputs(conn, &thread, &blob_dir))
            .await
            .map_err(|cause| PendingInputsError {
                thread: asked,
                cause: Box::new(cause),
            })
    }
}

fn answer(
    tx: &mut Tx<'_>,
    id: &ItemId,
    answers: Vec<Option<artisan_api::QuestionAnswer>>,
    blob_dir: &std::path::Path,
) -> Result<Item, AnswerFailure> {
    let old = read::item(tx, id)?.ok_or(AnswerFailure::NotFound)?;
    let mut item = old.clone();
    let ItemKind::Question {
        blocking: false,
        questions,
        answers: saved,
        ..
    } = &mut item.kind
    else {
        return Err(AnswerRefused::EngineStopped.into());
    };
    collection::validate_answers(tx, questions, &answers)?;
    let input_id = ItemId(format!("answer-{id}"));
    if item.status.is_terminal() {
        if !collection::answers_equal(tx, saved.as_ref(), &answers)? {
            return Err(AnswerRefused::AnsweredDifferently.into());
        }
        return read::item(tx, &input_id)?.ok_or(AnswerFailure::NotFound);
    }
    if item.status != Status::AwaitingUser {
        return Err(AnswerRefused::NotWaiting.into());
    }
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
                from_agent: None,
                sender: None,
            },
        },
        blob_dir,
    )?;
    tx.mark_activity(&input.thread_id, input.seq)?;
    Ok(input)
}

impl Tx<'_> {
    fn upsert_item(&mut self, mut new: NewItem) -> Result<Item, WriteFailure> {
        let native_id = new.native_id.clone();
        let old = read::item(self, &new.id)?;
        if let ItemKind::Agent { label, .. } = &mut new.kind
            && label.as_ref().is_none_or(|label| label.trim().is_empty())
        {
            *label = old.as_ref().and_then(|item| match &item.kind {
                ItemKind::Agent { label, .. } => label
                    .as_ref()
                    .filter(|label| !label.trim().is_empty())
                    .cloned(),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            });
            if label.is_none() {
                let taken = read::agent_labels(self, &new.thread_id)?;
                *label = Some(artisan_api::choose_agent_name(
                    new.id.as_str(),
                    taken.iter().map(String::as_str),
                ));
            }
        }
        let item = if let Some(old) = old {
            if old.thread_id != new.thread_id {
                return Err(WriteFailure::ItemMoved {
                    item: new.id,
                    stored: old.thread_id,
                    requested: new.thread_id,
                });
            }
            let mut kind = new.kind;
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
                run_id: old.run_id.clone().or(new.run_id),
                parent_id: new.parent_id.or_else(|| old.parent_id.clone()),
                status: old.next_status(new.status),
                created_at: old.created_at,
                updated_at: old.updated_at,
                kind,
            };
            self.save_item(item, Some(old))?
        } else {
            let created_at = self.created_at();
            self.save_item(fresh_item(new, created_at), None)?
        };
        self.set_native_id(&item.id, native_id.as_deref())?;
        Ok(item)
    }

    fn send_message(
        &mut self,
        thread_id: ThreadId,
        message: SendMessage,
        acceptance: Acceptance,
        blob_dir: &std::path::Path,
    ) -> Result<(Item, bool, EngineId), InputFailure> {
        let mut thread = read::thread(self, &thread_id)?
            .ok_or_else(|| SendRejected::ThreadNotFound(thread_id.clone()))?;
        let engine = thread.config.engine;
        let (item, created) = self.insert_input(
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
                    steer: false,
                    from_agent: acceptance.from_agent.clone(),
                    sender: acceptance.sender.clone(),
                },
            },
            blob_dir,
        )?;
        if created {
            if let Some(config) = message.config {
                if !valid_config(&config) {
                    return Err(SendRejected::InvalidConfig.into());
                }
                thread.config = config;
            }
            if thread.title.is_none() {
                thread.title = acceptance.title;
            }
            self.accept_command(thread, item.seq, acceptance.remember_config, message.draft)?;
        }
        Ok((item, created, engine))
    }
}
