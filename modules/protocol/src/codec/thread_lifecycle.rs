//! Where a thread lives: the archived-threads read, the reader's changes to
//! a thread, and the archived listing that answers both.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{
    ARCHIVED_THREADS_MAX, ArchivedThread, ArchivedThreadListing, ReadArchivedThreads, ThreadChange,
    UpdateThread,
};

use crate::artisan_capnp::{archived_thread_list, update_thread_request};

/// Encodes the archived-threads read.
pub(crate) fn encode_read_archived_threads(mut builder: request::Builder<'_>) {
    builder.set_read_archived_threads(());
}

/// Decodes the archived-threads read.
pub(crate) const fn decode_read_archived_threads() -> ClientRequest {
    ClientRequest::Query(Query::ReadArchivedThreads(ReadArchivedThreads))
}

/// Encodes one change to a thread.
pub(crate) fn encode_update_thread(builder: request::Builder<'_>, command: &UpdateThread) {
    let mut encoded = builder.init_update_thread();
    encoded.set_thread_id(command.thread_id.as_str());
    let mut change = encoded.init_change();
    match &command.change {
        ThreadChange::Rename(title) => change.set_rename(title.as_str()),
        ThreadChange::Settle => change.set_settle(()),
        ThreadChange::Archive => change.set_archive(()),
        ThreadChange::Unarchive => change.set_unarchive(()),
    }
}

/// Decodes one change to a thread.
pub(crate) fn decode_update_thread(
    request_id: RequestId,
    value: update_thread_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "request.updateThread.threadId")?,
        "request.updateThread.threadId",
    )?;
    let change = match value.get_change().which()? {
        update_thread_request::change::Rename(title) => ThreadChange::Rename(
            ThreadTitle::parse(read_text(title, "request.updateThread.rename")?)
                .map_err(|source| ProtocolDecodeError::ThreadTitle { source })?,
        ),
        update_thread_request::change::Settle(()) => ThreadChange::Settle,
        update_thread_request::change::Archive(()) => ThreadChange::Archive,
        update_thread_request::change::Unarchive(()) => ThreadChange::Unarchive,
    };
    Ok(ClientRequest::Command(Command::UpdateThread(
        UpdateThread {
            request_id,
            thread_id,
            change,
        },
    )))
}

/// Encodes the archived threads.
pub(crate) fn encode_archived_threads(
    builder: archived_thread_list::Builder<'_>,
    value: &ArchivedThreadListing,
) -> Result<(), ProtocolEncodeError> {
    let field = "archivedThreads.threads";
    let mut threads = builder.init_threads(list_length(field, value.threads().len())?);
    for (index, row) in value.threads().iter().enumerate() {
        let mut encoded = threads.reborrow().get(list_index(field, index)?);
        encode_recent_thread(encoded.reborrow().init_thread(), &row.thread);
        encoded.set_archived_at_ms(row.archived_at.as_millis());
    }
    Ok(())
}

/// Decodes the archived threads after checking their bound.
pub(crate) fn decode_archived_threads(
    value: archived_thread_list::Reader<'_>,
) -> Result<ArchivedThreadListing, ProtocolDecodeError> {
    let threads = value.get_threads()?;
    let count = threads.len() as usize;
    if count > ARCHIVED_THREADS_MAX {
        return Err(ProtocolDecodeError::ThreadListing {
            source: ThreadListingError::TooManyThreads {
                count,
                maximum: ARCHIVED_THREADS_MAX,
            },
        });
    }
    let rows = threads
        .iter()
        .map(|row| {
            Ok(ArchivedThread {
                thread: decode_recent_thread(row.get_thread()?)?,
                archived_at: UnixMillis::from_millis(row.get_archived_at_ms()),
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    ArchivedThreadListing::new(rows).map_err(|source| ProtocolDecodeError::ThreadListing { source })
}
