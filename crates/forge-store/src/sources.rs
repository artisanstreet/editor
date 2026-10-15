//! Large text and collections stored outside items, read in bounded ranges.

use crate::{Store, collection, content, source_index};
use anyhow::{Result, ensure};
use artisan_api::{Item, now};
use forge_content::ItemTexts as _;
use std::sync::Arc;

impl Store {
    /// Bounded immutable source range; does not load or parse an Item body.
    pub async fn content_range(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::ContentQuery,
    ) -> Result<Option<artisan_api::ContentRange>> {
        let id = id.clone();
        self.read(move |conn| content::range(conn, &id, query))
            .await
    }

    /// Pure immutable Markdown topology. Only this bounded range enters the wire.
    pub async fn source_index(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::SourceIndexQuery,
    ) -> Result<Option<artisan_api::SourceIndexRange>> {
        ensure!(id.is_valid(), "invalid source identity");
        ensure!(
            query
                .limit
                .is_none_or(|n| (1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&n)),
            "invalid source index page limit"
        );
        let (read_id, read_query) = (id.clone(), query.clone());
        if let Some(range) = self
            .read(move |conn| source_index::range(conn, &read_id, read_query))
            .await?
        {
            return Ok(Some(range));
        }
        // One build at a time: a build holds a whole source in memory. The
        // permit travels with the work, so a dropped request cannot let a
        // second build start while the first still runs.
        let permit = Arc::new(self.inner.index_build.clone().acquire_owned().await?);
        let (read_id, worker) = (id.clone(), permit.clone());
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
                self.write(move |tx| {
                    let _permit = permit;
                    source_index::persist(tx, prepared)
                })
                .await?;
            }
        }
        let id = id.clone();
        self.read(move |conn| source_index::range(conn, &id, query))
            .await
    }

    /// Indexed canonical collection records. Transcript reads never hydrate the
    /// complete collection, and each page has count and encoded-byte limits.
    pub async fn collection_range<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        id: &artisan_api::ContentId,
        query: artisan_api::CollectionQuery,
    ) -> Result<Option<artisan_api::CollectionRange<T>>> {
        let id = id.clone();
        self.read(move |conn| collection::range(conn, &id, query))
            .await
    }

    /// Resolve only an already-admitted input or explicit execution/history read.
    /// Transcript pages, events and bootstrap never call this method.
    pub async fn resolve_text(&self, text: &artisan_api::TextContent) -> Result<String> {
        let text = text.clone();
        self.read(move |conn| content::resolve(conn, &text)).await
    }

    pub async fn resolve_item_text(&self, mut item: Item) -> Result<Item> {
        self.read(move |conn| {
            collection::hydrate_item(conn, &mut item.kind)?;
            item.kind.try_visit_texts_mut(|text| {
                *text = content::resolve(conn, text)?.into();
                Ok::<_, anyhow::Error>(())
            })?;
            Ok(item)
        })
        .await
    }

    pub async fn gc_content(&self) -> Result<usize> {
        self.write(|tx| content::collect(tx, now())).await
    }
}
