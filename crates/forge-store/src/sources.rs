use crate::content::ContentError;
use crate::error::{StorageError, operation_error};
use crate::source_index::SourceIndexError;
use crate::{Store, collection, content, source_index};
use artisan_api::ContentId;
use std::sync::Arc;

operation_error!(ContentRangeError(ContentError), "read a range of content {content}", content: ContentId);
operation_error!(SourceIndexPageError(SourceIndexError), "read the index of source {content}", content: ContentId);
operation_error!(CollectionRangeError(ContentError), "read a range of collection {content}", content: ContentId);
operation_error!(ResolveTextError(ContentError), "resolve a stored text");
operation_error!(
    CollectContentError(StorageError),
    "collect unreferenced content"
);

impl Store {
    pub async fn content_range(
        &self,
        id: &ContentId,
        query: artisan_api::ContentQuery,
    ) -> Result<Option<artisan_api::ContentRange>, ContentRangeError> {
        let content = id.clone();
        let id = id.clone();
        self.read(move |conn| content::range(conn, &id, &query))
            .await
            .map_err(|cause| ContentRangeError {
                content,
                cause: Box::new(cause),
            })
    }

    pub async fn source_index(
        &self,
        id: &ContentId,
        query: artisan_api::SourceIndexQuery,
    ) -> Result<Option<artisan_api::SourceIndexRange>, SourceIndexPageError> {
        self.index_page(id, query)
            .await
            .map_err(|cause| SourceIndexPageError {
                content: id.clone(),
                cause: Box::new(cause),
            })
    }

    async fn index_page(
        &self,
        id: &ContentId,
        query: artisan_api::SourceIndexQuery,
    ) -> Result<Option<artisan_api::SourceIndexRange>, SourceIndexError> {
        if !id.is_valid() {
            return Err(SourceIndexError::InvalidIdentity {
                source_id: id.clone(),
            });
        }
        if let Some(limit) = query
            .limit
            .filter(|limit| !(1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(limit))
        {
            return Err(SourceIndexError::PageLimit { limit });
        }
        let (read_id, read_query) = (id.clone(), query.clone());
        if let Some(range) = self
            .read(move |conn| source_index::range(conn, &read_id, &read_query))
            .await?
        {
            return Ok(Some(range));
        }
        let permit = Arc::new(
            Arc::clone(&self.inner.index_build)
                .acquire_owned()
                .await
                .map_err(StorageError::ReadSlot)?,
        );
        let (read_id, worker) = (id.clone(), Arc::clone(&permit));
        let format = query.format;
        let built = self
            .read(move |conn| {
                let _permit = worker;
                source_index::prepare(conn, &read_id, format)
            })
            .await?;
        match built {
            source_index::Prepared::Missing => return Ok(None),
            source_index::Prepared::Indexed => {}
            source_index::Prepared::Built(prepared) => {
                if !self.persist_index(prepared, permit).await? {
                    return Ok(None);
                }
            }
        }
        let id = id.clone();
        self.read(move |conn| source_index::range(conn, &id, &query))
            .await
    }

    async fn persist_index(
        &self,
        prepared: source_index::PreparedIndex,
        permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<bool, SourceIndexError> {
        let prepared = Arc::new(prepared);
        for batch in 0..prepared.batches() {
            let staging = Arc::clone(&prepared);
            let staged = self
                .write(move |tx| source_index::stage(tx, &staging, batch))
                .await?;
            match staged {
                source_index::Staging::Staged => {}
                source_index::Staging::Published => return Ok(true),
                source_index::Staging::Gone => return Ok(false),
            }
        }
        self.write(move |tx| {
            let _permit = permit;
            source_index::publish(tx, &prepared)
        })
        .await
        .map(|published| published.is_some())
    }

    pub async fn collection_range<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        id: &ContentId,
        query: artisan_api::CollectionQuery,
    ) -> Result<Option<artisan_api::CollectionRange<T>>, CollectionRangeError> {
        let content = id.clone();
        let id = id.clone();
        self.read(move |conn| collection::range(conn, &id, query))
            .await
            .map_err(|cause| CollectionRangeError {
                content,
                cause: Box::new(cause),
            })
    }

    pub async fn resolve_text(
        &self,
        text: &artisan_api::TextContent,
    ) -> Result<String, ResolveTextError> {
        let text = text.clone();
        self.read(move |conn| content::resolve(conn, &text))
            .await
            .map_err(|cause| ResolveTextError {
                cause: Box::new(cause),
            })
    }

    pub async fn gc_content(&self) -> Result<usize, CollectContentError> {
        self.write(|tx| content::collect(tx, artisan_api::now()))
            .await
            .map_err(|cause| CollectContentError {
                cause: Box::new(cause),
            })
    }
}
