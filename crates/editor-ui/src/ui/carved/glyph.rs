use std::sync::Arc;

use aqua_core::DisplayItem;
use aqua_ui::element::{Canvas, Element};
use aqua_ui::style::Style;

use crate::ui::theme::Oklch;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Icon {
    Search,
    Gear,
    User,
    Info,
    Laptop,
    Messages,
    FileText,
    Plug,
    Tool,
    Close,
    Sun,
    Moon,
    Selector,
    Check,
    Logout,
    Download,
    ChevronDown,
    ChevronUp,
    Windows,
    Apple,
    Linux,
    Refresh,
    Pencil,
    Trash,
    Key,
    World,
    Terminal,
    FolderCode,
    Unarchive,
    Spinner,
    Plus,
}

enum Ink {
    Stroke(f32),
    Fill,
}

impl Icon {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per icon carries its prototype path data"
    )]
    const fn shape(self) -> (&'static str, Ink) {
        match self {
            Self::Search => (
                r#"<path d="M3 10a7 7 0 1 0 14 0a7 7 0 1 0 -14 0"/><path d="M21 21l-6 -6"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Gear => (
                r#"<path d="M10.325 4.317c.426 -1.756 2.924 -1.756 3.35 0a1.724 1.724 0 0 0 2.573 1.066c1.543 -.94 3.31 .826 2.37 2.37a1.724 1.724 0 0 0 1.065 2.572c1.756 .426 1.756 2.924 0 3.35a1.724 1.724 0 0 0 -1.066 2.573c.94 1.543 -.826 3.31 -2.37 2.37a1.724 1.724 0 0 0 -2.572 1.065c-.426 1.756 -2.924 1.756 -3.35 0a1.724 1.724 0 0 0 -2.573 -1.066c-1.543 .94 -3.31 -.826 -2.37 -2.37a1.724 1.724 0 0 0 -1.065 -2.572c-1.756 -.426 -1.756 -2.924 0 -3.35a1.724 1.724 0 0 0 1.066 -2.573c-.94 -1.543 .826 -3.31 2.37 -2.37c1 .608 2.296 .07 2.572 -1.065"/><path d="M9 12a3 3 0 1 0 6 0a3 3 0 0 0 -6 0"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::User => (
                r#"<path d="M8 7a4 4 0 1 0 8 0a4 4 0 0 0 -8 0"/><path d="M6 21v-2a4 4 0 0 1 4 -4h4a4 4 0 0 1 4 4v2"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Info => (
                r#"<path d="M3 12a9 9 0 1 0 18 0a9 9 0 0 0 -18 0"/><path d="M12 9h.01"/><path d="M11 12h1v4h1"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Laptop => (
                r#"<path d="M3 19l18 0"/><path d="M5 7a1 1 0 0 1 1 -1h12a1 1 0 0 1 1 1v8a1 1 0 0 1 -1 1h-12a1 1 0 0 1 -1 -1l0 -8"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Messages => (
                r#"<path d="M21 14l-3 -3h-7a1 1 0 0 1 -1 -1v-6a1 1 0 0 1 1 -1h9a1 1 0 0 1 1 1v10"/><path d="M14 15v2a1 1 0 0 1 -1 1h-7l-3 3v-10a1 1 0 0 1 1 -1h2"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::FileText => (
                r#"<path d="M14 3v4a1 1 0 0 0 1 1h4"/><path d="M17 21h-10a2 2 0 0 1 -2 -2v-14a2 2 0 0 1 2 -2h7l5 5v11a2 2 0 0 1 -2 2"/><path d="M9 9l1 0"/><path d="M9 13l6 0"/><path d="M9 17l6 0"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Plug => (
                r#"<path d="M7 12l5 5l-1.5 1.5a3.536 3.536 0 1 1 -5 -5l1.5 -1.5"/><path d="M17 12l-5 -5l1.5 -1.5a3.536 3.536 0 1 1 5 5l-1.5 1.5"/><path d="M3 21l2.5 -2.5"/><path d="M18.5 5.5l2.5 -2.5"/><path d="M10 11l-2 2"/><path d="M13 14l-2 2"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Tool => (
                r#"<path d="M7 10h3v-3l-3.5 -3.5a6 6 0 0 1 8 8l6 6a2 2 0 0 1 -3 3l-6 -6a6 6 0 0 1 -8 -8l3.5 3.5"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Close => (
                r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Sun => (
                r#"<path d="M8 12a4 4 0 1 0 8 0a4 4 0 1 0 -8 0"/><path d="M3 12h1m8 -9v1m8 8h1m-9 8v1m-6.4 -15.4l.7 .7m12.1 -.7l-.7 .7m0 11.4l.7 .7m-12.1 -.7l-.7 .7"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Moon => (
                r#"<path d="M12 3c.132 0 .263 0 .393 0a7.5 7.5 0 0 0 7.92 12.446a9 9 0 1 1 -8.313 -12.454z"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Selector => (
                r#"<path d="M8 9l4 -4l4 4"/><path d="M16 15l-4 4l-4 -4"/>"#,
                Ink::Stroke(2.0),
            ),
            Self::Check => (r#"<path d="M5 12l5 5l10 -10"/>"#, Ink::Stroke(2.2)),
            Self::Logout => (
                r#"<path d="M14 8v-2a2 2 0 0 0 -2 -2h-7a2 2 0 0 0 -2 2v12a2 2 0 0 0 2 2h7a2 2 0 0 0 2 -2v-2"/><path d="M9 12h12l-3 -3"/><path d="M18 15l3 -3"/>"#,
                Ink::Stroke(1.9),
            ),
            Self::Download => (
                r#"<path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-2"/><path d="M7 11l5 5l5 -5"/><path d="M12 4l0 12"/>"#,
                Ink::Stroke(2.0),
            ),
            Self::ChevronDown => (r#"<path d="m6 9 6 6 6-6"/>"#, Ink::Stroke(2.0)),
            Self::ChevronUp => (r#"<path d="m6 15 6-6 6 6"/>"#, Ink::Stroke(2.0)),
            Self::Windows => (
                r#"<path d="M3 5.55 10.35 4.5v7.05H3V5.55zm0 12.9 7.35 1.05V12.5H3v5.95zm8.35 1.2L21 21v-8.5h-9.65v7.15zm0-15.3v7.2H21V3l-9.65 1.35z"/>"#,
                Ink::Fill,
            ),
            Self::Apple => (
                r#"<path d="M16.365 1.43c0 1.14-.493 2.27-1.177 3.08-.744.9-1.99 1.57-2.987 1.57-.12 0-.23-.02-.3-.03-.01-.06-.04-.22-.04-.39 0-1.15.572-2.27 1.206-2.98.804-.94 2.142-1.64 3.248-1.68.03.13.05.28.05.43zm4.565 15.71c-.03.07-.463 1.58-1.518 3.12-.945 1.34-1.94 2.71-3.43 2.71-1.517 0-1.9-.88-3.63-.88-1.698 0-2.302.91-3.67.91-1.377 0-2.332-1.26-3.428-2.8-1.287-1.82-2.323-4.63-2.323-7.28 0-4.28 2.797-6.55 5.552-6.55 1.448 0 2.675.95 3.6.95.865 0 2.222-1.01 3.902-1.01.613 0 2.886.06 4.374 2.19-.13.09-2.383 1.37-2.383 4.19 0 3.26 2.854 4.42 2.955 4.45z"/>"#,
                Ink::Fill,
            ),
            Self::Linux => (
                r#"<path d="M12 3c-2.5 0-4 2-4 5 0 1.4.2 2.6-.7 3.8-.9 1.2-2.3 2.4-2.3 4.2 0 1.7 1.2 3 2.8 3h8.4c1.6 0 2.8-1.3 2.8-3 0-1.8-1.4-3-2.3-4.2-.9-1.2-.7-2.4-.7-3.8 0-3-1.5-5-4-5Z"/><circle cx="10.2" cy="7.5" r=".7" fill="currentColor" stroke="none"/><circle cx="13.8" cy="7.5" r=".7" fill="currentColor" stroke="none"/><path d="M10.5 10.2c.9.6 2.1.6 3 0"/><path d="M8.5 19v1.5M15.5 19v1.5"/>"#,
                Ink::Stroke(1.75),
            ),
            Self::Refresh => (
                r#"<path d="M21 12a9 9 0 1 1-2.64-6.36"/><polyline points="21 3 21 9 15 9"/>"#,
                Ink::Stroke(2.0),
            ),
            Self::Pencil => (
                r#"<path d="M4 20h4l10.5 -10.5a2.828 2.828 0 1 0 -4 -4l-10.5 10.5v4"/><path d="M13.5 6.5l4 4"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Trash => (
                r#"<path d="M4 7l16 0"/><path d="M10 11l0 6"/><path d="M14 11l0 6"/><path d="M5 7l1 12a2 2 0 0 0 2 2h8a2 2 0 0 0 2 -2l1 -12"/><path d="M9 7v-3a1 1 0 0 1 1 -1h4a1 1 0 0 1 1 1v3"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Key => (
                r#"<path d="M16.555 3.843l3.602 3.602a2.877 2.877 0 0 1 0 4.069l-2.643 2.643a2.877 2.877 0 0 1 -4.069 0l-.301 -.301l-6.558 6.558a2 2 0 0 1 -1.239 .578l-.175 .008h-1.172a1 1 0 0 1 -.993 -.883l-.007 -.117v-1.172a2 2 0 0 1 .467 -1.284l.119 -.13l.414 -.414h2v-2h2v-2l2.144 -2.144l-.301 -.301a2.877 2.877 0 0 1 0 -4.069l2.643 -2.643a2.877 2.877 0 0 1 4.069 0z"/><path d="M15 9h.01"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::World => (
                r#"<path d="M3 12a9 9 0 1 0 18 0a9 9 0 0 0 -18 0"/><path d="M3.6 9h16.8"/><path d="M3.6 15h16.8"/><path d="M11.5 3a17 17 0 0 0 0 18"/><path d="M12.5 3a17 17 0 0 1 0 18"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Terminal => (
                r#"<path d="M8 9l3 3l-3 3"/><path d="M13 15l3 0"/><path d="M3 4m0 2a2 2 0 0 1 2 -2h14a2 2 0 0 1 2 2v12a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2z"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::FolderCode => (
                r#"<path d="M11 19h-6a2 2 0 0 1 -2 -2v-11a2 2 0 0 1 2 -2h4l3 3h7a2 2 0 0 1 2 2v4"/><path d="M20 21l2 -2l-2 -2"/><path d="M17 17l-2 2l2 2"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Unarchive => (
                r#"<path d="M9 14l-4 -4l4 -4"/><path d="M5 10h11a4 4 0 1 1 0 8h-1"/>"#,
                Ink::Stroke(1.8),
            ),
            Self::Spinner => (
                r#"<circle cx="12" cy="12" r="8" opacity="0.22"/><path d="M12 4a8 8 0 0 1 8 8"/>"#,
                Ink::Stroke(2.2),
            ),
            Self::Plus => (r#"<path d="M12 5l0 14"/><path d="M5 12l14 0"/>"#, Ink::Stroke(1.8)),
        }
    }

    #[must_use]
    pub fn source(self) -> String {
        let (body, ink) = self.shape();
        let paint = match ink {
            Ink::Stroke(width) => format!(
                r#"fill="none" stroke="currentColor" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round""#
            ),
            Ink::Fill => r#"fill="currentColor""#.to_owned(),
        };
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {paint}>{body}</svg>"#)
    }
}

#[must_use]
pub fn glyph(icon: Icon, edge: f32, color: Oklch) -> Element {
    let source: Arc<str> = icon.source().into();
    let tint = color.to_reference_paint();
    Canvas::new()
        .width(edge)
        .height(edge)
        .style(Style::new().flex_shrink(0.0))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: Some(tint),
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}
