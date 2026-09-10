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
    ComposerStateCodecError, decode_failed_message_listing, decode_list_failed_messages_request,
    decode_list_queued_messages_request, decode_queued_message_listing,
    decode_queued_message_withdrawal_result, decode_read_recalled_message_request,
    decode_read_run_usage_request, decode_recalled_message_result, decode_run_usage_result,
    decode_withdraw_queued_message_request, encode_failed_message_listing,
    encode_list_failed_messages_request, encode_list_queued_messages_request,
    encode_queued_message_listing, encode_queued_message_withdrawal_result,
    encode_read_recalled_message_request, encode_read_run_usage_request,
    encode_recalled_message_result, encode_run_usage_result,
    encode_withdraw_queued_message_request, validate_recalled_message_scope,
    validate_run_usage_scope, validate_withdrawal_response_correlation,
};
