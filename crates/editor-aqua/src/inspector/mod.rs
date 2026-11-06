mod controller;
mod dialog;
mod model;
mod owner;
mod palette;
mod terminal;
mod terminal_view;
mod view;

pub use model::{Entry, entries};
pub use terminal::{
    Feed, ShellPrompt, StreamState, accepts_item, command_lines, shell_output, shell_prompt,
};

pub use controller::Inspector;

pub use dialog::{Job, Layout, ListChange, Row, TerminalDialog, list_changes};

pub use palette::TerminalPalette;

pub use terminal_view::{TerminalAction, terminal_view};

pub use view::{InspectorAction, inspector_view};

pub use owner::{InspectorElements, InspectorOwner, OwnerAction, SourceIds, SourceTarget};
