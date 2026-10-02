//! The recent-threads read, its answer, and the pushed listing.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{
    RECENT_PROJECT_ICON_MAX_BYTES, RECENT_THREADS_MAX, ReadRecentThreads, RecentProjectIcon,
    RecentThread, RecentThreadListing,
};

/// Encodes the recent-threads read.
pub(crate) fn encode_recent_threads_request(mut builder: request::Builder<'_>) {
    builder.set_read_recent_threads(());
}

/// Decodes the recent-threads read.
pub(crate) const fn decode_recent_threads_request() -> ClientRequest {
    ClientRequest::Query(Query::ReadRecentThreads(ReadRecentThreads))
}

/// Encodes one recent-threads listing, shared by the answer and the push.
pub(crate) fn encode_recent_threads(
    builder: artisan_capnp::recent_thread_list::Builder<'_>,
    value: &RecentThreadListing,
) -> Result<(), ProtocolEncodeError> {
    let field = "recentThreads.threads";
    let mut threads = builder.init_threads(list_length(field, value.threads().len())?);
    for (index, row) in value.threads().iter().enumerate() {
        let mut encoded = threads.reborrow().get(list_index(field, index)?);
        encode_thread(encoded.reborrow().init_thread(), &row.thread);
        encoded.set_subtitle(row.subtitle.as_str());
        if let Some(host) = &row.project_icon.host {
            encoded.set_repository_host(host.as_str());
            if row.project_icon.png.len() <= RECENT_PROJECT_ICON_MAX_BYTES {
                encoded.set_project_icon_png(&row.project_icon.png);
            }
        }
    }
    Ok(())
}

/// Decodes one recent-threads listing after checking its bound.
pub(crate) fn decode_recent_threads(
    value: artisan_capnp::recent_thread_list::Reader<'_>,
) -> Result<RecentThreadListing, ProtocolDecodeError> {
    let threads = value.get_threads()?;
    let count = threads.len() as usize;
    if count > RECENT_THREADS_MAX {
        return Err(ProtocolDecodeError::ThreadListing {
            source: ThreadListingError::TooManyThreads {
                count,
                maximum: RECENT_THREADS_MAX,
            },
        });
    }
    let field = "recentThreads.subtitle";
    let rows = threads
        .iter()
        .map(|row| {
            let host = read_text(row.get_repository_host(), "recentThreads.repositoryHost")?;
            let png = row.get_project_icon_png()?;
            Ok(RecentThread {
                thread: decode_thread(row.get_thread()?)?,
                subtitle: DisplayName::parse(read_text(row.get_subtitle(), field)?)
                    .map_err(|source| ProtocolDecodeError::DisplayName { field, source })?,
                project_icon: RecentProjectIcon {
                    host: if host.is_empty() {
                        None
                    } else {
                        Some(DisplayName::parse(&host).map_err(|source| {
                            ProtocolDecodeError::DisplayName {
                                field: "recentThreads.repositoryHost",
                                source,
                            }
                        })?)
                    },
                    png: if !host.is_empty() && png.len() <= RECENT_PROJECT_ICON_MAX_BYTES {
                        png.into()
                    } else {
                        Default::default()
                    },
                },
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    RecentThreadListing::new(rows).map_err(|source| ProtocolDecodeError::ThreadListing { source })
}
