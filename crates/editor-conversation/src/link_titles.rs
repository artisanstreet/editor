//! Page titles for the links the transcript shows, so a bare URL reads as
//! the page it points to.
//!
//! Rows look titles up while they render. A link without one is asked of
//! the open host once ([`Conversation::fetch_link_titles`]), and the rows
//! that showed it re-render when the answer arrives. A link whose page has
//! no title, or that failed, keeps its own label and isn't asked again. The
//! table keeps the newest [`MAX_LINKS`] URLs.

use crate::Conversation;
use editor_ui::markdown_renderer::RichLinkTitleSource;
use gpui::{Context, EntityId, SharedString};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

const MAX_LINKS: usize = 256;

#[derive(Default)]
pub(crate) struct LinkTitles {
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    links: HashMap<String, Link>,
    /// Oldest first, for eviction.
    order: VecDeque<String>,
    /// Asked for, not yet sent to the host.
    queued: Vec<String>,
}

enum Link {
    /// Asked; these rows show it and re-render with the answer.
    Pending(Vec<EntityId>),
    Titled(SharedString),
    Untitled,
}

impl LinkTitles {
    /// Lookups for one row's render. With `asker`, missing titles are
    /// queued for that row.
    pub(crate) fn for_row(&self, asker: Option<EntityId>) -> RowTitles<'_> {
        RowTitles {
            titles: self,
            asker,
        }
    }

    pub(crate) fn has_queued(&self) -> bool {
        !self.state.borrow().queued.is_empty()
    }

    fn take_queued(&self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().queued)
    }

    /// Records the answer for `url`; returns the rows waiting on it.
    fn answer(&self, url: &str, title: Option<String>) -> Vec<EntityId> {
        let link = title
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty())
            .map_or(Link::Untitled, |title| Link::Titled(title.into()));
        let mut state = self.state.borrow_mut();
        let Some(known) = state.links.get_mut(url) else {
            // Evicted while it was asked: keep the answer anyway.
            state.insert(url.to_owned(), link);
            return Vec::new();
        };
        match std::mem::replace(known, link) {
            Link::Pending(rows) => rows,
            Link::Titled(_) | Link::Untitled => Vec::new(),
        }
    }

    /// Drops a link that couldn't be asked, so it is asked again later.
    fn forget(&self, url: &str) {
        let mut state = self.state.borrow_mut();
        state.links.remove(url);
        state.order.retain(|known| known != url);
    }
}

impl State {
    fn insert(&mut self, url: String, link: Link) {
        while self.links.len() >= MAX_LINKS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.links.remove(&oldest);
        }
        self.order.push_back(url.clone());
        self.links.insert(url, link);
    }
}

/// One row's view of the table, handed to the markdown renderer.
pub(crate) struct RowTitles<'a> {
    titles: &'a LinkTitles,
    asker: Option<EntityId>,
}

impl RichLinkTitleSource for RowTitles<'_> {
    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        let mut state = self.titles.state.borrow_mut();
        match state.links.get_mut(destination) {
            Some(Link::Titled(title)) => Some(title.clone()),
            Some(Link::Untitled) => None,
            Some(Link::Pending(rows)) => {
                if let Some(row) = self.asker
                    && !rows.contains(&row)
                {
                    rows.push(row);
                }
                None
            }
            None => {
                if let Some(row) = self.asker {
                    state.insert(destination.to_owned(), Link::Pending(vec![row]));
                    state.queued.push(destination.to_owned());
                }
                None
            }
        }
    }
}

impl Conversation {
    /// Asks the open host for the titles rows queued while rendering.
    pub(crate) fn fetch_link_titles(&mut self, cx: &mut Context<Self>) {
        let urls = self.shared.links.take_queued();
        if urls.is_empty() {
            return;
        }
        let Some(host) = self.host(cx) else {
            for url in &urls {
                self.shared.links.forget(url);
            }
            return;
        };
        let client = host.read(cx).client.clone();
        for url in urls {
            let client = client.clone();
            cx.spawn(async move |this, cx| {
                let title = match client.link_title(&url).await {
                    Ok(answer) => answer.title,
                    Err(error) => {
                        tracing::debug!(url, "couldn't get the link's title: {error:#}");
                        None
                    }
                };
                let _ = this.update(cx, |this, cx| this.link_title_arrived(&url, title, cx));
            })
            .detach();
        }
    }

    fn link_title_arrived(&mut self, url: &str, title: Option<String>, cx: &mut Context<Self>) {
        let rows = self.shared.links.answer(url, title);
        for (index, view) in self.row_views.iter().enumerate() {
            if rows.contains(&view.entity_id()) {
                view.update(cx, |view, cx| {
                    view.measured = None;
                    cx.notify();
                });
                self.list.remeasure_items(index..index + 1);
            }
        }
    }
}
