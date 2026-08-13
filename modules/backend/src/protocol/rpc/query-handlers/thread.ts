import { Effect, Option } from "effect";

import type {
	ConversationQueryEnvelope,
	ConversationQueryResultEnvelope,
	MessageImageAttachmentQueryEnvelope,
	MessageImageAttachmentQueryResultEnvelope,
	ProtocolErrorDetail,
	ThreadListQueryEnvelope,
	ThreadListQueryResultEnvelope,
	ModelFavoritesQueryEnvelope,
	ModelFavoritesQueryResultEnvelope,
	ThreadRetentionQueryEnvelope,
	ThreadRetentionQueryResultEnvelope,
	ThreadSessionQueryEnvelope,
	ThreadSessionQueryResultEnvelope,
	ThreadTranscriptQueryEnvelope,
	ThreadTranscriptQueryResultEnvelope,
	ThreadWorkQueryEnvelope,
	ThreadWorkQueryResultEnvelope,
	ThreadOpenQueryEnvelope,
	ThreadOpenQueryResultEnvelope,
} from "@artisan/protocol";

import { ConversationReadModel } from "../../../conversation/index.ts";
import { OrchestrationRepository } from "../../../persistence/orchestration/repository";
import { RuntimeMetadata } from "../../../runtime/metadata";
import { ModelFavoritesService } from "../../../model-favorites/service";
import { ThreadRetentionPolicyService } from "../../../threads/thread-retention-policy";
import { ThreadReadModel } from "../../../persistence/thread-read-model";
import { TranscriptReadModel } from "../../../persistence/transcript-read-model";

export type ThreadQueryEnvelope =
	| ConversationQueryEnvelope
	| MessageImageAttachmentQueryEnvelope
	| ThreadListQueryEnvelope
	| ModelFavoritesQueryEnvelope
	| ThreadRetentionQueryEnvelope
	| ThreadSessionQueryEnvelope
	| ThreadTranscriptQueryEnvelope
	| ThreadWorkQueryEnvelope
	| ThreadOpenQueryEnvelope;

export type ThreadQueryResultEnvelope =
	| ConversationQueryResultEnvelope
	| MessageImageAttachmentQueryResultEnvelope
	| ThreadListQueryResultEnvelope
	| ModelFavoritesQueryResultEnvelope
	| ThreadRetentionQueryResultEnvelope
	| ThreadSessionQueryResultEnvelope
	| ThreadTranscriptQueryResultEnvelope
	| ThreadWorkQueryResultEnvelope
	| ThreadOpenQueryResultEnvelope;

const ProjectionUnavailable = (message: string): ProtocolErrorDetail => ({
	code: "projection.unavailable",
	message,
	retryable: true,
});

const RetentionUnavailable: ProtocolErrorDetail = {
	code: "retention.unavailable",
	message: "The thread retention policy could not be read.",
	retryable: true,
};

const FavoritesUnavailable: ProtocolErrorDetail = {
	code: "model_favorites.unavailable",
	message: "The starred models could not be read.",
	retryable: true,
};

export const MakeThreadQueryHandler = Effect.gen(function* () {
	const conversation_read_model = yield* ConversationReadModel;
	const metadata = yield* RuntimeMetadata;
	const orchestration = yield* OrchestrationRepository;
	const model_favorites = yield* ModelFavoritesService;
	const retention_policy = yield* ThreadRetentionPolicyService;
	const thread_read_model = yield* ThreadReadModel;
	const transcript_read_model = yield* TranscriptReadModel;

	const Envelope = <Kind extends ThreadQueryResultEnvelope["kind"], Payload>(
		query: ThreadQueryEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			return {
				correlation_id: query.message_id,
				kind,
				message_id,
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at,
			};
		});

	const handlers = {
		"conversation.query": (query: ConversationQueryEnvelope) =>
			conversation_read_model.ReadSnapshot(query.payload.thread_id).pipe(
				Effect.mapError(() =>
					ProjectionUnavailable("The conversation projection could not be read."),
				),
				Effect.flatMap((availability) =>
					availability.status === "available"
						? Envelope(query, "conversation.query.result", availability.snapshot)
						: Effect.fail(
								ProjectionUnavailable(
									"The conversation projection is unavailable.",
								),
							),
				),
			),
		"message.image_attachment.query": (query: MessageImageAttachmentQueryEnvelope) =>
			conversation_read_model.ReadImageAttachment(query.payload).pipe(
				Effect.flatMap((attachment) =>
					Envelope(
						query,
						"message.image_attachment.query.result",
						Option.match(attachment, {
							onNone: () => ({ status: "not_found" as const }),
							onSome: (value) => ({
								attachment: value,
								status: "found" as const,
							}),
						}),
					),
				),
				Effect.mapError(() =>
					ProjectionUnavailable("The message image attachment could not be read."),
				),
			),
		"thread.list.query": (query: ThreadListQueryEnvelope) =>
			thread_read_model.Snapshot().pipe(
				Effect.flatMap((snapshot) => Envelope(query, "thread.list.query.result", snapshot)),
				Effect.mapError(() =>
					ProjectionUnavailable("The thread projection could not be read."),
				),
			),
		"model.favorites.query": (query: ModelFavoritesQueryEnvelope) =>
			model_favorites.Read.pipe(
				Effect.flatMap((favorites) =>
					Envelope(query, "model.favorites.query.result", favorites),
				),
				Effect.mapError(() => FavoritesUnavailable),
			),
		"thread.retention.query": (query: ThreadRetentionQueryEnvelope) =>
			retention_policy.Read.pipe(
				Effect.flatMap((policy) =>
					Envelope(query, "thread.retention.query.result", policy),
				),
				Effect.mapError(() => RetentionUnavailable),
			),
		"thread.session.query": (query: ThreadSessionQueryEnvelope) =>
			orchestration.GetSession(query.payload.thread_id).pipe(
				Effect.flatMap((snapshot) =>
					Envelope(query, "thread.session.query.result", snapshot),
				),
				Effect.mapError(() =>
					ProjectionUnavailable("The thread session could not be read."),
				),
			),
		"thread.transcript.query": (query: ThreadTranscriptQueryEnvelope) =>
			transcript_read_model.Read(query.payload).pipe(
				Effect.flatMap((snapshot) =>
					Envelope(query, "thread.transcript.query.result", snapshot),
				),
				Effect.mapError(() =>
					ProjectionUnavailable("The thread transcript could not be read."),
				),
			),
		"thread.work.query": (query: ThreadWorkQueryEnvelope) =>
			orchestration.GetWork(query.payload.thread_id).pipe(
				Effect.flatMap((work) =>
					Envelope(query, "thread.work.query.result", work ? { work } : {}),
				),
				Effect.mapError(() =>
					ProjectionUnavailable("The thread work projection could not be read."),
				),
			),
		"thread.open.query": (query: ThreadOpenQueryEnvelope) =>
			Effect.gen(function* () {
				const exact_thread = yield* thread_read_model.Lookup(query.payload.thread_id);
				const thread = Option.isSome(exact_thread)
					? exact_thread
					: query.payload.thread_id.startsWith("thread_")
						? Option.none()
						: yield* thread_read_model.Lookup(`thread_${query.payload.thread_id}`);
				if (Option.isNone(thread)) {
					return yield* Effect.fail(
						ProjectionUnavailable("The thread projection is unavailable."),
					);
				}
				const [session, work, availability] = yield* Effect.all(
					[
						orchestration.GetSession(thread.value.thread_id),
						orchestration.GetWork(thread.value.thread_id),
						conversation_read_model.ReadSnapshot(thread.value.thread_id),
					],
					{ concurrency: "unbounded" },
				);
				if (availability.status !== "available") {
					return yield* Effect.fail(
						ProjectionUnavailable("The thread projection is unavailable."),
					);
				}
				return yield* Envelope(query, "thread.open.query.result", {
					conversation: availability.snapshot,
					session,
					thread: thread.value,
					...(work ? { work } : {}),
				});
			}).pipe(
				Effect.mapError(() =>
					ProjectionUnavailable("The thread projection could not be read."),
				),
			),
	};

	return (
		query: ThreadQueryEnvelope,
	): Effect.Effect<ThreadQueryResultEnvelope, ProtocolErrorDetail> => {
		switch (query.kind) {
			case "conversation.query":
				return handlers["conversation.query"](query);
			case "message.image_attachment.query":
				return handlers["message.image_attachment.query"](query);
			case "thread.list.query":
				return handlers["thread.list.query"](query);
			case "model.favorites.query":
				return handlers["model.favorites.query"](query);
			case "thread.retention.query":
				return handlers["thread.retention.query"](query);
			case "thread.session.query":
				return handlers["thread.session.query"](query);
			case "thread.transcript.query":
				return handlers["thread.transcript.query"](query);
			case "thread.work.query":
				return handlers["thread.work.query"](query);
			case "thread.open.query":
				return handlers["thread.open.query"](query);
		}
	};
});
