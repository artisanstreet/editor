//! Public protocol seam for queued-message recall and run-usage state.
//!
//! The parent protocol owns envelope framing and union dispatch. These
//! re-exports let that parent and backend callers use one stable vocabulary
//! while the generated-reader adapters remain isolated in
//! [`crate::composer_state_codec`].

pub use artisan_domain::composer_state::{
    COMPOSER_STATE_IMAGE_MAX_BYTES, COMPOSER_STATE_IMAGE_MAX_COUNT,
    COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES, ComposerStateValueError, QueuedMessageWithdrawalResult,
    ReadRecalledMessage, ReadRunUsage, RecalledMessageResult, RunUsageResult,
    WithdrawQueuedMessageCommand, validate_payload_bounds,
};
pub use artisan_domain::{
    FAILED_MESSAGE_LIST_MAX, FailedMessageListing, FailedMessageSummary, ListFailedMessages,
    ListQueuedMessages, QueuedMessageListOrder, QueuedMessageListing, QueuedMessageSummary,
};

pub use crate::composer_state_codec::{
    ComposerStateCodecError, decode_composer_attachment_result,
    decode_composer_attachment_uploaded, decode_composer_draft_result, decode_composer_draft_saved,
    decode_failed_message_listing, decode_failed_message_recovered, decode_failed_message_retried,
    decode_failed_message_target, decode_list_failed_messages_request,
    decode_list_queued_messages_request, decode_message_outbox,
    decode_queue_stored_message_request, decode_queued_message_listing,
    decode_queued_message_withdrawal_result, decode_read_composer_attachment_request,
    decode_read_composer_draft_request, decode_read_recalled_message_request,
    decode_read_run_usage_request, decode_recalled_message_result, decode_run_usage_result,
    decode_save_composer_draft_request, decode_upload_composer_attachment_request,
    decode_withdraw_queued_message_request, encode_composer_attachment_result,
    encode_composer_attachment_uploaded, encode_composer_draft_result, encode_composer_draft_saved,
    encode_failed_message_listing, encode_failed_message_recovered, encode_failed_message_retried,
    encode_failed_message_target, encode_list_failed_messages_request,
    encode_list_queued_messages_request, encode_message_outbox,
    encode_queue_stored_message_request, encode_queued_message_listing,
    encode_queued_message_withdrawal_result, encode_read_composer_attachment_request,
    encode_read_composer_draft_request, encode_read_recalled_message_request,
    encode_read_run_usage_request, encode_recalled_message_result, encode_run_usage_result,
    encode_save_composer_draft_request, encode_upload_composer_attachment_request,
    encode_withdraw_queued_message_request, validate_recalled_message_scope,
    validate_run_usage_scope, validate_withdrawal_response_correlation,
};
