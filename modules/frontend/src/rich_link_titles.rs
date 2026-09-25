//! Bounded frontend retention of resolved rich-link page titles.
//!
//! The Markdown renderer asks this table synchronously for one display title
//! per openable HTTP(S) link. A miss is recorded as a pending resolve request
//! exactly once, so streaming re-renders never duplicate transport work. A
//! resolved title is shown until a refreshed resolution replaces it; a failed
//! resolution keeps the authored label and is not retried for this session.
//!
//! The table is deliberately transport-free: hosts drain pending requests and
//! feed results back through [`RichLinkTitleTable::resolve`] and
//! [`RichLinkTitleTable::fail`].

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};

use artisan_ui::markdown_renderer::RichLinkTitleSource;
use gpui::SharedString;

use crate::rich_link_url::rich_link_metadata_url;

/// Maximum number of URLs retained by one surface title table.
pub const MAX_RETAINED_RICH_LINK_TITLES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
enum RichLinkTitleEntry {
    /// A resolve request is in flight; the authored label stays visible.
    Pending,
    /// A resolved display title with the backend cache expiry.
    Resolved {
        title: SharedString,
        expires_at_ms: i64,
    },
    /// Resolution failed; the authored label stays and is not retried.
    Failed,
}

/// Per-surface resolved-title retention with pending and failure states.
///
/// Keys are canonical absolute HTTP(S) URLs produced by the shared
/// [`rich_link_metadata_url`] policy with the fragment removed, mirroring the
/// reference controller and Forge's own resolution key. A destination
/// carrying a fragment therefore resolves to the same entry the transport
/// sends.
#[derive(Default)]
pub struct RichLinkTitleTable {
    entries: HashMap<String, RichLinkTitleEntry>,
    order: VecDeque<String>,
    icons: HashMap<String, std::sync::Arc<gpui::RenderImage>>,
}

/// Canonicalizes one destination into the title table's cache key.
///
/// Applies the shared absolute-HTTP(S) policy and then removes the fragment,
/// mirroring the reference controller's canonical URL and Forge's own
/// resolution key, so a fragment-carrying destination shares one entry.
fn canonical_rich_link_title_url(destination: &str) -> Option<String> {
    let url = rich_link_metadata_url(Some(destination))?;
    let mut parsed = url::Url::parse(&url).ok()?;
    parsed.set_fragment(None);
    Some(parsed.as_str().to_owned())
}

impl RichLinkTitleTable {
    /// Creates one empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the display title for one destination, if one is retained.
    ///
    /// A pending or failed entry keeps the authored label (`None`); a
    /// resolved entry stays visible even after its backend expiry so a
    /// refresh never flickers the label back.
    #[must_use]
    pub fn lookup(&self, destination: &str) -> Option<SharedString> {
        let url = canonical_rich_link_title_url(destination)?;
        match self.entries.get(&url)? {
            RichLinkTitleEntry::Resolved { title, .. } => Some(title.clone()),
            RichLinkTitleEntry::Pending | RichLinkTitleEntry::Failed => None,
        }
    }

    /// Returns the canonical URL that needs a resolve request right now.
    #[must_use]
    pub fn request_candidate(&self, destination: &str, now_ms: i64) -> Option<String> {
        let url = canonical_rich_link_title_url(destination)?;
        match self.entries.get(&url) {
            None => Some(url),
            Some(RichLinkTitleEntry::Resolved { expires_at_ms, .. })
                if *expires_at_ms <= now_ms =>
            {
                Some(url)
            }
            Some(
                RichLinkTitleEntry::Pending
                | RichLinkTitleEntry::Failed
                | RichLinkTitleEntry::Resolved { .. },
            ) => None,
        }
    }

    /// Marks one canonical URL pending exactly once.
    ///
    /// Returns the canonical URL when this call newly queued it, and `None`
    /// when the entry is already pending, failed, or fresh.
    pub fn queue(&mut self, url: &str, now_ms: i64) -> Option<String> {
        let canonical = canonical_rich_link_title_url(url)?;
        self.request_candidate(&canonical, now_ms)?;
        self.insert(canonical.clone(), RichLinkTitleEntry::Pending);
        Some(canonical)
    }

    /// Records one resolved title with its backend cache expiry.
    ///
    /// Empty titles are ignored so a degenerate resolution can never erase
    /// the authored label.
    pub fn resolve(&mut self, url: &str, title: SharedString, expires_at_ms: i64) {
        if title.trim().is_empty() {
            return;
        }
        let Some(canonical) = canonical_rich_link_title_url(url) else {
            return;
        };
        self.insert(
            canonical,
            RichLinkTitleEntry::Resolved {
                title,
                expires_at_ms,
            },
        );
    }

    /// Records one failed resolution; the authored label stays visible.
    pub fn fail(&mut self, url: &str) {
        let Some(canonical) = canonical_rich_link_title_url(url) else {
            return;
        };
        self.insert(canonical, RichLinkTitleEntry::Failed);
    }

    /// Retains a decoded, bounded favicon without making render-time HTTP requests.
    pub fn resolve_icon(&mut self, destination: &str, bytes: &[u8]) {
        let Some(key) = canonical_rich_link_title_url(destination) else {
            return;
        };
        if !self.entries.contains_key(&key) {
            return;
        }
        self.icons.remove(&key);
        if bytes.is_empty() || bytes.len() > 65_536 {
            return;
        }
        if let Some(icon) = decode_svg_favicon(bytes) {
            self.icons.insert(key, icon);
            return;
        }
        let Ok(mut reader) =
            image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()
        else {
            return;
        };
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(512);
        limits.max_image_height = Some(512);
        limits.max_alloc = Some(4 * 1024 * 1024);
        reader.limits(limits);
        let Ok(decoded) = reader.decode() else {
            return;
        };
        let mut pixels = decoded.thumbnail(32, 32).into_rgba8();
        // GPUI consumes BGRA pixels.
        for pixel in pixels.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        let frame = image::Frame::new(pixels);
        self.icons.insert(
            key,
            std::sync::Arc::new(gpui::RenderImage::new(vec![frame])),
        );
    }

    /// Returns an already decoded icon for one canonical destination.
    #[must_use]
    pub fn icon(&self, destination: &str) -> Option<std::sync::Arc<gpui::RenderImage>> {
        self.icons
            .get(&canonical_rich_link_title_url(destination)?)
            .cloned()
    }

    fn insert(&mut self, url: String, entry: RichLinkTitleEntry) {
        self.entries.insert(url.clone(), entry);
        if let Some(position) = self.order.iter().position(|key| key == &url) {
            self.order.remove(position);
        }
        self.order.push_back(url);
        while self.entries.len() > MAX_RETAINED_RICH_LINK_TITLES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
            self.icons.remove(&oldest);
        }
    }

    /// Test-only retained-entry count.
    #[cfg(test)]
    fn retained(&self) -> usize {
        self.entries.len()
    }
}

/// Rasterizes uncompressed SVGs into a fixed 32px image. No external or embedded
/// images are loaded; SVGs cannot access the editor's filesystem or network.
fn decode_svg_favicon(bytes: &[u8]) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let source = std::str::from_utf8(bytes).ok()?;
    if !source
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        return None;
    }
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(source, &options).ok()?;
    let scale = (32.0 / tree.size().width()).min(32.0 / tree.size().height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(32, 32)?;
    let transform = resvg::tiny_skia::Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (32.0 - tree.size().width() * scale) / 2.0,
        (32.0 - tree.size().height() * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let bytes = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.blue(), color.green(), color.red(), color.alpha()]
        })
        .collect();
    let pixels = image::RgbaImage::from_raw(32, 32, bytes)?;
    Some(std::sync::Arc::new(gpui::RenderImage::new(vec![
        image::Frame::new(pixels),
    ])))
}

impl RichLinkTitleSource for RichLinkTitleTable {
    fn favicon(&self, destination: &str) -> Option<std::sync::Arc<gpui::RenderImage>> {
        self.icon(destination)
    }
    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        self.lookup(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(value: &str) -> SharedString {
        SharedString::from(value.to_owned())
    }

    #[test]
    fn queue_is_deduplicated_per_canonical_url() {
        let mut table = RichLinkTitleTable::new();
        assert_eq!(
            table.queue("https://example.com/page", 0),
            Some("https://example.com/page".to_owned())
        );
        assert_eq!(table.queue("https://example.com/page", 0), None);
        assert_eq!(table.queue("https://example.com/page#section", 0), None);
        assert!(table.lookup("https://example.com/page").is_none());

        // A fragment resolves against the same canonical entry too.
        table.resolve("https://example.com/page", title("Resolved"), 1_000);
        assert_eq!(
            table.lookup("https://example.com/page#section"),
            Some(title("Resolved"))
        );
    }

    #[test]
    fn expired_titles_requeue_then_fall_back_while_pending() {
        let mut table = RichLinkTitleTable::new();
        table.resolve("https://example.com/page", title("Resolved"), 1_000);
        assert_eq!(
            table.request_candidate("https://example.com/page", 999),
            None
        );
        assert_eq!(
            table.lookup("https://example.com/page"),
            Some(title("Resolved"))
        );

        let candidate = table
            .request_candidate("https://example.com/page", 1_000)
            .expect("expired entry needs a refresh");
        assert_eq!(candidate, "https://example.com/page");
        assert_eq!(
            table.queue("https://example.com/page", 1_000),
            Some("https://example.com/page".to_owned())
        );
        assert_eq!(table.queue("https://example.com/page", 1_001), None);
        assert!(table.lookup("https://example.com/page").is_none());
    }

    #[test]
    fn failures_keep_the_authored_label_and_are_not_retried() {
        let mut table = RichLinkTitleTable::new();
        assert!(table.queue("https://example.com/page", 0).is_some());
        table.fail("https://example.com/page");
        assert!(table.lookup("https://example.com/page").is_none());
        assert_eq!(table.queue("https://example.com/page", 0), None);
        assert_eq!(table.request_candidate("https://example.com/page", 0), None);
    }

    #[test]
    fn table_is_bounded_and_evicts_oldest_entries() {
        let mut table = RichLinkTitleTable::new();
        for index in 0..(MAX_RETAINED_RICH_LINK_TITLES + 5) {
            let url = format!("https://example.com/{index}");
            assert!(table.queue(&url, 0).is_some());
            table.resolve(&url, title(&format!("Title {index}")), 10_000);
        }
        assert_eq!(table.retained(), MAX_RETAINED_RICH_LINK_TITLES);
        assert!(table.lookup("https://example.com/0").is_none());
        assert_eq!(
            table.lookup(&format!(
                "https://example.com/{}",
                MAX_RETAINED_RICH_LINK_TITLES + 4
            )),
            Some(title(&format!(
                "Title {}",
                MAX_RETAINED_RICH_LINK_TITLES + 4
            )))
        );
        assert!(table.queue("https://example.com/0", 0).is_some());
    }

    #[test]
    fn non_http_destinations_are_never_queued_or_resolved() {
        let mut table = RichLinkTitleTable::new();
        assert_eq!(table.queue("mailto:user@example.com", 0), None);
        assert_eq!(table.queue("/relative/path", 0), None);
        assert_eq!(table.queue("ftp://example.com/file", 0), None);
        table.resolve("mailto:user@example.com", title("Email"), 1_000);
        assert!(table.lookup("mailto:user@example.com").is_none());
        assert_eq!(table.retained(), 0);
    }

    #[test]
    fn empty_resolved_titles_do_not_erase_the_label() {
        let mut table = RichLinkTitleTable::new();
        assert!(table.queue("https://example.com/page", 0).is_some());
        table.resolve("https://example.com/page", title("   "), 1_000);
        assert!(table.lookup("https://example.com/page").is_none());
        assert_eq!(table.queue("https://example.com/page", 0), None);
    }
}

#[cfg(test)]
mod favicon_tests {
    use super::*;
    #[test]
    fn decoded_icons_are_bounded_and_share_fragment_cache_keys() {
        let mut table = RichLinkTitleTable::new();
        let url = "https://example.com/page";
        table.resolve(url, SharedString::from("Example"), 1000);
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(16, 16)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        table.resolve_icon(url, png.get_ref());
        assert!(table.icon("https://example.com/page#section").is_some());
        let rendered = artisan_ui::markdown_renderer::present_inline_with_titles(
            &[artisan_ui::markdown::Span::Text(
                "See https://example.com/page#section.".into(),
            )],
            artisan_ui::theme::ArtisanTheme::for_mode(artisan_ui::theme::ThemeMode::Dark),
            &table,
        );
        assert_eq!(rendered.source, "See \u{2003}\u{2060}\u{00a0}Example.");
        assert_eq!(rendered.icon_offsets[0].0, 4);
        assert_eq!(
            rendered.links[0].destination,
            "https://example.com/page#section"
        );
        assert_eq!(
            &rendered.source[rendered.links[0].range.clone()],
            "\u{2003}\u{2060}\u{00a0}Example"
        );
        table.resolve_icon(url, b"not an image");
        assert!(table.icon(url).is_none());
        assert_eq!(table.lookup(url).as_deref(), Some("Example"));
        let mut huge = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(513, 1)
            .write_to(&mut huge, image::ImageFormat::Png)
            .unwrap();
        table.resolve_icon(url, huge.get_ref());
        assert!(table.icon(url).is_none());
    }
}

#[cfg(test)]
mod svg_favicon_tests {
    use super::*;
    #[test]
    fn svg_icons_are_fixed_size_and_keep_their_color() {
        let icon = decode_svg_favicon(br##"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="10000"><path fill="#ff0000" d="M0 0h10000v10000H0z"/></svg>"##).unwrap();
        assert_eq!(icon.size(0).width.0, 32);
        assert_eq!(&icon.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert!(decode_svg_favicon(b"garbage").is_none());
    }
}
