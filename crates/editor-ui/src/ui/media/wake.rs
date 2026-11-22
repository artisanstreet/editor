use std::sync::Arc;

use aqua::mailbox::{Mailbox, SendError, Wake};

pub(crate) fn wake_on(mailbox: &Mailbox<()>) -> Wake {
    let sender = mailbox.sender();
    Arc::new(move || match sender.send(()) {
        Ok(()) | Err(SendError::Full(()) | SendError::Closed(())) => {}
    })
}
