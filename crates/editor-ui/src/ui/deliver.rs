use editor_core::delivery::DeliverySender;
use tokio::sync::mpsc::error::SendError;

pub(crate) async fn deliver<T>(sender: &DeliverySender<T>, message: T) {
    match sender.send(message).await {
        Ok(()) | Err(SendError(_)) => {}
    }
}
