import type { ProjectionEnvelope, ProjectionSubscription } from "./model";

/**
 * Advances only cursor-enabled conversation subscriptions. Updates are already
 * accepted into the queue before this runs, and that queue survives a transport
 * reset, so a replacement session can safely resume after the newest delivery.
 * Callers that omitted a cursor retain the legacy fresh-snapshot behavior.
 */
export const advance_conversation_cursor = (
	subscription: ProjectionSubscription,
	envelope: ProjectionEnvelope,
): ProjectionSubscription => {
	if (
		subscription._tag !== "conversation" ||
		subscription.envelope.payload.type !== "conversation" ||
		subscription.envelope.payload.cursor === undefined ||
		(envelope.kind !== "conversation.snapshot" && envelope.kind !== "conversation.patch")
	) {
		return subscription;
	}

	return {
		...subscription,
		envelope: {
			...subscription.envelope,
			payload: {
				...subscription.envelope.payload,
				cursor: {
					conversation_id: envelope.payload.conversation_id,
					last_patch_sequence:
						envelope.kind === "conversation.snapshot"
							? envelope.payload.last_patch_sequence
							: envelope.payload.to_sequence,
				},
			},
		},
	};
};
