mod controller;
mod dialog;
mod model;
mod owner;
mod palette;
mod terminal;
mod view;

pub use model::Entry;
pub(crate) use model::entries;
pub use terminal::{Feed, ShellPrompt, StreamState, command_lines, shell_prompt};
pub(crate) use terminal::{accepts_item, shell_output};

pub use controller::Inspector;

pub(crate) use dialog::Row;
pub use dialog::{Job, Layout, ListChange, TerminalDialog, list_changes};

pub use palette::TerminalPalette;

pub use terminal::TerminalAction;
pub(crate) use terminal::terminal_view;

pub(crate) use view::inspector_view;
pub use view::{AgentCard, InspectorAction, agent_card, render_agent_card};

pub use owner::{
    BuildInspectorError, InspectorElements, InspectorOwner, OwnerAction, SourceIds, SourceTarget,
};
