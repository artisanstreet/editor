import { Cause, Effect, Layer, Option, Queue, Ref, Stream } from "effect";

import {
	type CommandEnvelope,
	type OrchestrationGraphQueryEnvelope,
	type TerminalListQueryEnvelope,
	type ThreadListQueryEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type ThreadWorkQueryEnvelope,
} from "@artisan/protocol";

import {
	ArtisanClient,
	type ArtisanClientError,
	type ArtisanClientOptions,
	type ArtisanCommandInput,
	type ArtisanCommandReceipt,
	type ArtisanThreadRetentionUpdateInput,
} from "../client-contract";
import { TransportRuntime } from "../transport-runtime";
import { client_error, validate_client_options } from "./client-common";
import { make_client_connection_lifecycle } from "./client-connection";
import { make_client_request_coordinator } from "./client-request-coordinator";
import { make_client_stream_channel } from "./client-stream-channel";
import { make_client_subscription_coordinator } from "./client-subscription-coordinator";

/** Builds the public client service from focused lifecycle coordinators. */
export function make_artisan_client_layer(input_options: ArtisanClientOptions = {}) {
	const options: Required<ArtisanClientOptions> = {
		error_capacity: input_options.error_capacity ?? 64,
		event_capacity: input_options.event_capacity ?? 256,
		max_pending_requests: input_options.max_pending_requests ?? 128,
		reconnect_delay_ms: input_options.reconnect_delay_ms ?? 50,
		stream_capacity: input_options.stream_capacity ?? 64,
		subscription_capacity: input_options.subscription_capacity ?? 64,
	};

	return Layer.effect(
		ArtisanClient,
		Effect.gen(function* () {
			if (!validate_client_options(options)) {
				return yield* Effect.fail(
					client_error(
						"configuration",
						"Artisan client limits are invalid.",
						new Error("client limits must be bounded safe integers"),
					),
				);
			}

			const runtime = yield* TransportRuntime;
			const errors = yield* Effect.acquireRelease(
				Queue.dropping<ArtisanClientError, Cause.Done<void>>(options.error_capacity),
				Queue.shutdown,
			);
			const disposed = yield* Ref.make(false);
			const connection = yield* make_client_connection_lifecycle(options.reconnect_delay_ms);

			const publish_error = (error: ArtisanClientError) =>
				Effect.sync(() => {
					Queue.offerUnsafe(errors, error);
				});
			const requests = yield* make_client_request_coordinator(
				options.max_pending_requests,
				connection.SendCurrent,
			);
			const subscriptions = yield* make_client_subscription_coordinator(
				options.event_capacity,
				options.subscription_capacity,
				connection.MakeTrace,
				runtime.MakeId,
				connection.SendCurrent,
				publish_error,
			);
			const streams = yield* make_client_stream_channel(
				options.stream_capacity,
				runtime.MakeId,
				connection.AwaitActive,
				connection.Current,
			);

			const shutdown = (failure: Option.Option<ArtisanClientError>) =>
				Effect.uninterruptible(
					Effect.gen(function* () {
						const should_close = yield* Ref.getAndSet(disposed, true);

						if (should_close) {
							return;
						}

						const terminal_error = Option.getOrElse(failure, () =>
							client_error(
								"disposed",
								"The Artisan client was disposed.",
								new Error("client disposed"),
							),
						);

						yield* requests.Dispose(terminal_error);
						yield* subscriptions.Dispose(failure);
						yield* streams.Dispose(failure);
						yield* connection.Dispose;

						if (Option.isSome(failure)) {
							yield* publish_error(failure.value);
						}

						yield* Queue.end(errors);
					}),
				);

			yield* Effect.addFinalizer(() => shutdown(Option.none()));
			yield* connection.Start({
				on_fatal: (error) => shutdown(Option.some(error)),
				publish_error,
				requests,
				streams,
				subscriptions,
			});

			const command = (input: ArtisanCommandInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const command_id = input.command_id ?? trace.message_id;
					const envelope: CommandEnvelope = {
						...trace,
						message_id: command_id,
						kind: "command",
						payload: input.payload,
						thread_id: input.thread_id,
						...(input.agent_id ? { agent_id: input.agent_id } : {}),
						...(input.causation_id ? { causation_id: input.causation_id } : {}),
						...(input.run_id ? { run_id: input.run_id } : {}),
					};
					const result = yield* requests.Request(envelope, "command.receipt");

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

			const list_threads = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: ThreadListQueryEnvelope = {
					...trace,
					kind: "thread.list.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "thread.list.query.result");

				return result.kind === "thread.list.query.result"
					? result.payload.threads
					: yield* Effect.die("thread list response narrowed incorrectly");
			});

			const get_thread_retention_policy = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: ThreadRetentionQueryEnvelope = {
					...trace,
					kind: "thread.retention.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "thread.retention.query.result");

				return result.kind === "thread.retention.query.result"
					? result.payload
					: yield* Effect.die("thread retention response narrowed incorrectly");
			});

			const update_thread_retention_policy = (input: ArtisanThreadRetentionUpdateInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
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
					const result = yield* requests.Request(envelope, "command.receipt");

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

			const get_thread_work = (thread_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ThreadWorkQueryEnvelope = {
						...trace,
						kind: "thread.work.query",
						payload: { thread_id },
					};
					const result = yield* requests.Request(envelope, "thread.work.query.result");

					return result.kind === "thread.work.query.result"
						? Option.fromUndefinedOr(result.payload.work)
						: yield* Effect.die("thread work response narrowed incorrectly");
				});

			const get_orchestration_graph = (group_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: OrchestrationGraphQueryEnvelope = {
						...trace,
						kind: "orchestration.graph.query",
						payload: { group_id },
					};
					const result = yield* requests.Request(
						envelope,
						"orchestration.graph.query.result",
					);

					return result.kind === "orchestration.graph.query.result"
						? result.payload.graph
						: yield* Effect.die("orchestration graph response narrowed incorrectly");
				});

			const list_terminals = (thread_id: string, workspace_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: TerminalListQueryEnvelope = {
						...trace,
						kind: "terminal.list.query",
						payload: { thread_id, workspace_id },
					};
					const result = yield* requests.Request(envelope, "terminal.list.query.result");

					return result.kind === "terminal.list.query.result"
						? result.payload.terminals
						: yield* Effect.die("terminal list response narrowed incorrectly");
				});

			return {
				Command: command,
				Cursors: subscriptions.Cursors,
				Dispose: shutdown(Option.none()),
				Errors: Stream.fromQueue(errors),
				Events: subscriptions.Events,
				GetOrchestrationGraph: get_orchestration_graph,
				GetThreadRetentionPolicy: get_thread_retention_policy,
				GetThreadWork: get_thread_work,
				ListTerminals: list_terminals,
				ListThreads: list_threads,
				OpenAsset: (asset_id) => streams.Open(`asset:${asset_id}`),
				OpenTerminalOutput: (terminal_id) => streams.Open(`terminal:${terminal_id}`),
				SubscribeOrchestrationGraph: subscriptions.SubscribeOrchestrationGraph,
				SubscribeThreadList: subscriptions.SubscribeThreadList,
				UpdateThreadRetentionPolicy: update_thread_retention_policy,
			};
		}),
	);
}
