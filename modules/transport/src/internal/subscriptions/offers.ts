import { Queue } from "effect";
import type {
	ConversationUpdate,
	OrchestrationGraphUpdate,
	ProjectCatalogUpdate,
	ThreadListUpdate,
	ThreadTranscriptUpdate,
	ThreadWorkUpdate,
} from "../../client-api/service";
import type { ProjectionEnvelope, ProjectionOffer, ProjectionSubscription } from "./model";

export const offer_projection_update = (
	subscription: ProjectionSubscription,
	envelope: ProjectionEnvelope,
): ProjectionOffer => {
	if (
		subscription._tag === "thread.list" &&
		(envelope.kind === "thread.list.snapshot" ||
			envelope.kind === "thread.list.upsert" ||
			envelope.kind === "thread.list.remove")
	) {
		const update: ThreadListUpdate =
			envelope.kind === "thread.list.snapshot"
				? {
						journal_sequence: envelope.journal_sequence,
						threads: envelope.payload.threads,
						type: "snapshot",
					}
				: envelope.kind === "thread.list.upsert"
					? {
							journal_sequence: envelope.journal_sequence,
							thread: envelope.payload,
							type: "upsert",
						}
					: {
							journal_sequence: envelope.journal_sequence,
							thread_id: envelope.payload.thread_id,
							type: "remove",
						};

		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}

	if (
		subscription._tag === "project.list" &&
		(envelope.kind === "project.list.snapshot" || envelope.kind === "project.list.updated")
	) {
		const update: ProjectCatalogUpdate = {
			snapshot: envelope.payload,
			type: envelope.kind === "project.list.snapshot" ? "snapshot" : "replacement",
		};
		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}

	if (
		subscription._tag === "orchestration.graph" &&
		(envelope.kind === "orchestration.graph.snapshot" ||
			envelope.kind === "orchestration.graph.patch")
	) {
		const update: OrchestrationGraphUpdate = {
			graph: envelope.payload.graph,
			journal_sequence: envelope.journal_sequence,
			type: envelope.kind === "orchestration.graph.snapshot" ? "snapshot" : "patch",
		};

		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}

	if (
		subscription._tag === "thread.transcript" &&
		(envelope.kind === "thread.transcript.snapshot" ||
			envelope.kind === "thread.transcript.append")
	) {
		const update: ThreadTranscriptUpdate =
			envelope.kind === "thread.transcript.snapshot"
				? {
						type: "snapshot",
						journal_sequence: envelope.journal_sequence,
						transcript: envelope.payload,
					}
				: {
						type: "append",
						journal_sequence: envelope.journal_sequence,
						entries: envelope.payload.entries,
					};
		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}

	if (
		subscription._tag === "conversation" &&
		(envelope.kind === "conversation.snapshot" || envelope.kind === "conversation.patch")
	) {
		const update: ConversationUpdate =
			envelope.kind === "conversation.snapshot"
				? { type: "snapshot", snapshot: envelope.payload }
				: { type: "patch", batch: envelope.payload };

		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}

	if (
		subscription._tag === "orchestration.group.list" &&
		(envelope.kind === "orchestration.group.list.snapshot" ||
			envelope.kind === "orchestration.group.list.patch")
	) {
		return Queue.offerUnsafe(subscription.queue, {
			type: envelope.kind === "orchestration.group.list.snapshot" ? "snapshot" : "patch",
			snapshot: envelope.payload,
		})
			? "offered"
			: "overflow";
	}
	if (subscription._tag === "thread.session" && envelope.kind === "thread.session.snapshot") {
		return Queue.offerUnsafe(subscription.queue, {
			type: "snapshot",
			snapshot: envelope.payload,
		})
			? "offered"
			: "overflow";
	}
	if (subscription._tag === "thread.work" && envelope.kind === "thread.work.snapshot") {
		const update: ThreadWorkUpdate = { type: "snapshot", snapshot: envelope.payload };
		return Queue.offerUnsafe(subscription.queue, update) ? "offered" : "overflow";
	}
	if (subscription._tag === "surface.list" && envelope.kind === "surface.list.snapshot") {
		return Queue.offerUnsafe(subscription.queue, {
			type: "snapshot",
			snapshot: envelope.payload,
		})
			? "offered"
			: "overflow";
	}
	if (
		subscription._tag === "surface.usage.aggregate" &&
		envelope.kind === "surface.usage.aggregate.snapshot"
	) {
		return Queue.offerUnsafe(subscription.queue, {
			type: "snapshot",
			snapshot: envelope.payload,
		})
			? "offered"
			: "overflow";
	}
	if (
		subscription._tag === "workspace.conflict.list" &&
		envelope.kind === "workspace.conflict.list.snapshot"
	) {
		return Queue.offerUnsafe(subscription.queue, {
			type: "snapshot",
			snapshot: envelope.payload,
		})
			? "offered"
			: "overflow";
	}

	return "mismatch";
};
