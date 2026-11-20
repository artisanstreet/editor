mod access;
mod editor;
mod render;
mod view;

pub use aqua_a11y_uia::accesskit::Role;
pub use aqua_a11y_uia::accesskit::{Action, ActionData, Point};
pub use aqua_core::{Key, Modifiers, NamedKey};
pub use editor::{Editor, Opening, Scenario};
pub use editor_aqua::core::SettingsPage;
pub use view::{Found, Scrolled, View};
#[cfg(windows)]
mod uia;
#[cfg(windows)]
pub use uia::{NativeEditor, Uia};
