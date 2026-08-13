import { Effect, Option } from "effect";

import {
	type ConversationQueryEnvelope,
	type CommandEnvelope,
	type MessageImageAttachmentQueryEnvelope,
	type ModelFavoriteUpdateEnvelope,
	type ModelFavoritesQueryEnvelope,
	type OrchestrationGraphQueryEnvelope,
	type OrchestrationGroupListQueryEnvelope,
	type SessionDefaultsQueryEnvelope,
	type SessionDefaultsUpdateEnvelope,
	type SessionDefaultsUpdateInput,
	type SurfaceListQueryEnvelope,
	type SurfaceUsageAggregateQueryEnvelope,
	type SurfaceUsageDailyQueryEnvelope,
	type TerminalListQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadSessionQueryEnvelope,
	type ThreadTranscriptQueryEnvelope,
	type ThreadWorkQueryEnvelope,
	type ThreadOpenQueryEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanCommandReceipt,
	ArtisanModelFavoriteUpdateInput,
	ArtisanThreadRetentionUpdateInput,
	ArtisanThreadSessionPolicyUpdateInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

/** Constructs thread policy, projection, orchestration, surface, and terminal operations. */
export const MakeThreadOperationsApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const get_thread_retention_policy = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const envelope: ThreadRetentionQueryEnvelope = {
			...trace,
			kind: "thread.retention.query",
			payload: {},
		};
		return (yield* context.Request(envelope)).payload;
	});
	const get_model_favorites = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const envelope: ModelFavoritesQueryEnvelope = {
			...trace,
			kind: "model.favorites.query",
			payload: {},
		};
		return (yield* context.Request(envelope)).payload;
	});
	const update_thread_retention_policy = (input: ArtisanThreadRetentionUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const command_id = input.command_id ?? trace.message_id;
			const envelope: ThreadRetentionUpdateEnvelope = {
				...trace,
				message_id: command_id,
				kind: "thread.retention.update",
				payload: {
					enabled: input.enabled,
					inactivity_days: input.inactivity_days,
				},
			};
			const result = yield* context.Request(envelope);

			if (result.kind !== "command.receipt") {
				return yield* Effect.die("thread retention receipt narrowed incorrectly");
			}

			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}

			return {
				command_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const update_thread_session_policy = (input: ArtisanThreadSessionPolicyUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const command_id = input.command_id ?? trace.message_id;
			const envelope: CommandEnvelope = {
				...trace,
				kind: "command",
				message_id: command_id,
				payload: { type: "thread.session_policy.update", policy: input.policy },
				thread_id: input.thread_id,
			};
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt") {
				return yield* Effect.die("command response narrowed incorrectly");
			}
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});

	const get_session_defaults = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const envelope: SessionDefaultsQueryEnvelope = {
			...trace,
			kind: "session.defaults.query",
			payload: {},
		};
		const result = yield* context.Request(envelope);
		return result.kind === "session.defaults.query.result"
			? result.payload
			: yield* Effect.die("session defaults response narrowed incorrectly");
	});

	const update_session_defaults = (input: SessionDefaultsUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: SessionDefaultsUpdateEnvelope = {
				...trace,
				kind: "session.defaults.update",
				payload: input,
			};
			const result = yield* context.Request(envelope);

			if (result.kind !== "command.receipt") {
				return yield* Effect.die("session defaults receipt narrowed incorrectly");
			}
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id: trace.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});

	const update_model_favorite = (input: ArtisanModelFavoriteUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const command_id = input.command_id ?? trace.message_id;
			const envelope: ModelFavoriteUpdateEnvelope = {
				...trace,
				message_id: command_id,
				kind: "model.favorite.update",
				payload: {
					favorite: input.favorite,
					model_id: input.model_id,
				},
			};
			const result = yield* context.Request(envelope);

			if (result.kind !== "command.receipt") {
				return yield* Effect.die("model favorite receipt narrowed incorrectly");
			}
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});

	const get_thread_work = (thread_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ThreadWorkQueryEnvelope = {
				...trace,
				kind: "thread.work.query",
				payload: { thread_id },
			};
			const result = yield* context.Request(envelope);

			return result.kind === "thread.work.query.result"
				? Option.fromUndefinedOr(result.payload.work)
				: yield* Effect.die("thread work response narrowed incorrectly");
		});

	const get_orchestration_graph = (group_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: OrchestrationGraphQueryEnvelope = {
				...trace,
				kind: "orchestration.graph.query",
				payload: { group_id },
			};
			const result = yield* context.Request(envelope);

			return result.kind === "orchestration.graph.query.result"
				? result.payload.graph
				: yield* Effect.die("orchestration graph response narrowed incorrectly");
		});

	const get_thread_transcript = (input: import("@artisan/protocol").ThreadTranscriptQuery) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ThreadTranscriptQueryEnvelope = {
				...trace,
				kind: "thread.transcript.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "thread.transcript.query.result"
				? result.payload
				: yield* Effect.die("thread transcript response narrowed incorrectly");
		});

	const get_conversation = (input: import("@artisan/protocol").ConversationQuery) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ConversationQueryEnvelope = {
				...trace,
				kind: "conversation.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);

			return result.kind === "conversation.query.result"
				? result.payload
				: yield* Effect.die("conversation response narrowed incorrectly");
		});
	const get_thread_open = (thread_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ThreadOpenQueryEnvelope = {
				...trace,
				kind: "thread.open.query",
				payload: { thread_id },
			};
			const result = yield* context.Request(envelope);
			return result.kind === "thread.open.query.result"
				? result.payload
				: yield* Effect.die("thread open response narrowed incorrectly");
		});

	const get_message_image_attachment = (
		input: import("@artisan/protocol").MessageImageAttachmentQuery,
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: MessageImageAttachmentQueryEnvelope = {
				...trace,
				kind: "message.image_attachment.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);

			if (result.kind !== "message.image_attachment.query.result") {
				return yield* Effect.die("message image attachment response narrowed incorrectly");
			}

			return result.payload.status === "found"
				? Option.some(result.payload.attachment)
				: Option.none();
		});

	const list_orchestration_groups = (thread_id: string, include_terminal: boolean) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: OrchestrationGroupListQueryEnvelope = {
				...trace,
				kind: "orchestration.group.list.query",
				payload: { thread_id, include_terminal },
			};
			const result = yield* context.Request(envelope);
			return result.kind === "orchestration.group.list.query.result"
				? result.payload
				: yield* Effect.die("orchestration group list response narrowed incorrectly");
		});

	const get_thread_session = (thread_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ThreadSessionQueryEnvelope = {
				...trace,
				kind: "thread.session.query",
				payload: { thread_id },
			};
			const result = yield* context.Request(envelope);
			return result.kind === "thread.session.query.result"
				? result.payload
				: yield* Effect.die("thread session response narrowed incorrectly");
		});

	const list_surface_items = (input: import("@artisan/protocol").SurfaceListQuery) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: SurfaceListQueryEnvelope = {
				...trace,
				kind: "surface.list.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "surface.list.query.result"
				? result.payload
				: yield* Effect.die("surface list response narrowed incorrectly");
		});

	const get_surface_usage_aggregate = (
		input: import("@artisan/protocol").SurfaceUsageAggregateQuery,
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: SurfaceUsageAggregateQueryEnvelope = {
				...trace,
				kind: "surface.usage.aggregate.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "surface.usage.aggregate.query.result"
				? result.payload
				: yield* Effect.die("surface usage response narrowed incorrectly");
		});

	const get_surface_usage_daily = (input: import("@artisan/protocol").SurfaceUsageDailyQuery) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: SurfaceUsageDailyQueryEnvelope = {
				...trace,
				kind: "surface.usage.daily.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "surface.usage.daily.query.result"
				? result.payload
				: yield* Effect.die("daily surface usage response narrowed incorrectly");
		});

	const list_terminals = (thread_id: string, workspace_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: TerminalListQueryEnvelope = {
				...trace,
				kind: "terminal.list.query",
				payload: { thread_id, workspace_id },
			};
			const result = yield* context.Request(envelope);

			return result.kind === "terminal.list.query.result"
				? result.payload.terminals
				: yield* Effect.die("terminal list response narrowed incorrectly");
		});

	return {
		get_conversation,
		get_thread_open,
		get_message_image_attachment,
		get_orchestration_graph,
		get_model_favorites,
		get_session_defaults,
		get_surface_usage_aggregate,
		get_surface_usage_daily,
		get_thread_session,
		get_thread_retention_policy,
		get_thread_transcript,
		get_thread_work,
		list_orchestration_groups,
		list_surface_items,
		list_terminals,
		update_model_favorite,
		update_session_defaults,
		update_thread_retention_policy,
		update_thread_session_policy,
	};
});
