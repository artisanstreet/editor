import { Buffer } from "node:buffer";

import {
	Cause,
	Deferred,
	Effect,
	Exit,
	Queue,
	Ref,
	Scope,
	Schema,
	Semaphore,
	Stream,
} from "effect";

import { EngineProcessError } from "../engine";
import {
	CodexAppServerClosedError,
	CodexAppServerConfigurationError,
	CodexAppServerProtocolError,
	CodexAppServerRequestTimeoutError,
	CodexAppServerResponseError,
	CodexAppServerSerializationError,
	type CodexInitializeResponse,
	type CodexJsonRpcResponseEnvelope,
	type CodexJsonRpcSuccessEnvelope,
	type CodexRequestId,
	DecodeCodexInboundEnvelope,
	DecodeCodexInitializeResponse,
	codex_opt_out_notification_methods,
	make_codex_initialize_request,
	make_codex_server_response,
} from "./protocol";
import {
	CodexJsonlFramer,
	CodexJsonlMalformedLineError,
	CodexJsonlOversizedLineError,
	type CodexJsonlDecode,
	type CodexJsonlFrame,
} from "./jsonl";
import { CodexProcessFactory, type CodexProcessSpawnInput } from "./process";

/** Configures one scoped Codex app-server process and its transport framing. @since 0.3.0 */
export interface CodexAppServerSessionOptions {
	readonly max_frame_bytes?: number;
	readonly request_timeout_ms?: number;
	readonly spawn: CodexProcessSpawnInput;
}

/** Supplies client identity for the Codex initialize/initialized handshake. @since 0.3.0 */
export interface CodexAppServerHandshakeInput {
	readonly client_name: string;
	readonly client_version: string;
}

const codex_opt_out_notification_method_set: ReadonlySet<string> = new Set(
	codex_opt_out_notification_methods,
);

/** Preserves a server notification or server-initiated request in receive order. @since 0.3.0 */
export interface CodexAppServerNotification {
	readonly frame_sequence: number;
	readonly id?: CodexRequestId;
	readonly method: string;
	readonly params?: unknown;
	readonly payload: unknown;
	readonly raw_frame_base64: string;
}

/** Captures process and protocol diagnostics independently from control frames. @since 0.3.0 */
export interface CodexAppServerDiagnostic {
	readonly frame_sequence?: number;
	readonly level: "error" | "info" | "warning";
	readonly message: string;
	readonly raw_frame_base64?: string;
	readonly source: "process" | "stderr" | "stdout";
}

/** Defines the failure union produced by a live scoped Codex app-server session. @since 0.3.0 */
export type CodexAppServerSessionFailure =
	| CodexAppServerClosedError
	| CodexAppServerConfigurationError
	| CodexAppServerProtocolError
	| CodexAppServerRequestTimeoutError
	| CodexAppServerResponseError
	| CodexAppServerSerializationError
	| EngineProcessError;

/** Owns one Codex app-server process and its correlated JSON-RPC conversation. @since 0.3.0 */
export interface CodexAppServerSession {
	readonly Close: Effect.Effect<void>;
	readonly Diagnostics: Stream.Stream<CodexAppServerDiagnostic>;
	readonly Handshake: (
		input: CodexAppServerHandshakeInput,
	) => Effect.Effect<CodexInitializeResponse, CodexAppServerSessionFailure>;
	readonly Notifications: Stream.Stream<CodexAppServerNotification>;
	readonly Notify: (
		method: string,
		params?: unknown,
	) => Effect.Effect<void, CodexAppServerSessionFailure>;
	readonly Request: (
		method: string,
		params: unknown,
		timeout_ms?: number,
	) => Effect.Effect<CodexJsonRpcSuccessEnvelope, CodexAppServerSessionFailure>;
	readonly Respond: (
		id: CodexRequestId,
		result: unknown,
	) => Effect.Effect<void, CodexAppServerSessionFailure>;
}

interface PendingRequest {
	readonly deferred: Deferred.Deferred<CodexJsonRpcSuccessEnvelope, CodexAppServerSessionFailure>;
	readonly method: string;
}

interface SessionState {
	readonly closed: boolean;
	readonly close_requested: boolean;
	readonly next_frame_sequence: number;
	readonly next_request_id: number;
	readonly pending: ReadonlyMap<CodexRequestId, PendingRequest>;
}

interface FinishResult {
	readonly pending: ReadonlyArray<PendingRequest>;
	readonly should_finish: boolean;
}

type SerializationOperation = CodexAppServerSerializationError["operation"];

const DecodeJsonValue = Schema.decodeUnknownEffect(Schema.Json, {
	onExcessProperty: "error",
});

function serialization_message(cause: unknown) {
	return cause instanceof Error ? cause.message : "Value cannot be encoded as JSON";
}

function EncodeJsonLine(payload: unknown, operation: SerializationOperation) {
	return Effect.gen(function* () {
		const serialized = yield* Effect.try({
			try: () => {
				const text = JSON.stringify(payload);

				if (text === undefined) {
					throw new Error("JSON.stringify returned undefined");
				}

				return text;
			},
			catch: (cause) =>
				new CodexAppServerSerializationError({
					message: serialization_message(cause),
					operation,
				}),
		});

		yield* DecodeJsonValue(payload).pipe(
			Effect.mapError(
				() =>
					new CodexAppServerSerializationError({
						message: "Outbound payload is not a lossless JSON value",
						operation,
					}),
			),
		);

		return new TextEncoder().encode(`${serialized}\n`);
	});
}

function ValidatePositiveOption(option: string, value: number) {
	return Number.isInteger(value) && value > 0
		? Effect.void
		: Effect.fail(new CodexAppServerConfigurationError({ option, value }));
}

/**
 * Opens one scoped Codex app-server process with ordered JSONL framing and
 * correlated JSON-RPC requests. The process is finalized with its parent scope.
 *
 * @since 0.3.0
 * @param options - Spawn input plus frame and request limits.
 * @returns A live app-server session backed by exactly one child process.
 */
export const open_codex_app_server_session = (
	options: CodexAppServerSessionOptions,
): Effect.Effect<
	CodexAppServerSession,
	CodexAppServerConfigurationError | EngineProcessError,
	CodexProcessFactory | Scope.Scope
> => {
	const max_frame_bytes = options.max_frame_bytes ?? 8 * 1_024 * 1_024;
	const request_timeout_ms = options.request_timeout_ms ?? 10_000;

	return Effect.gen(function* () {
		yield* ValidatePositiveOption("max_frame_bytes", max_frame_bytes);
		yield* ValidatePositiveOption("request_timeout_ms", request_timeout_ms);

		const factory = yield* CodexProcessFactory;
		const handle = yield* factory.Spawn(options.spawn);
		const diagnostics = yield* Queue.unbounded<CodexAppServerDiagnostic, Cause.Done>();
		const notification_ingress = yield* Queue.unbounded<
			CodexAppServerNotification,
			Cause.Done
		>();
		const notifications = yield* Queue.unbounded<CodexAppServerNotification, Cause.Done>();
		const close_complete = yield* Deferred.make<void>();
		const stdout_drained = yield* Deferred.make<
			void,
			CodexAppServerProtocolError | EngineProcessError
		>();
		const state = yield* Ref.make<SessionState>({
			closed: false,
			close_requested: false,
			next_frame_sequence: 0,
			next_request_id: 0,
			pending: new Map(),
		});
		const write_lock = yield* Semaphore.make(1);

		const OfferDiagnostic = (diagnostic: CodexAppServerDiagnostic) =>
			Queue.offer(diagnostics, diagnostic).pipe(Effect.asVoid);
		const EmitDiagnostic = (diagnostic: CodexAppServerDiagnostic) =>
			Effect.gen(function* () {
				const current = yield* Ref.get(state);

				if (current.closed) {
					return;
				}

				yield* OfferDiagnostic(diagnostic);
			});
		const RemovePending = (id: CodexRequestId) =>
			Ref.modify(state, (current) => {
				const pending = current.pending.get(id);

				if (!pending) {
					return [undefined, current] as const;
				}

				const next_pending = new Map(current.pending);

				next_pending.delete(id);

				return [pending, { ...current, pending: next_pending }] as const;
			});
		const Finish = (
			failure: CodexAppServerSessionFailure,
			diagnostic: CodexAppServerDiagnostic,
		) =>
			Effect.gen(function* () {
				const completion = yield* Ref.modify(
					state,
					(current): readonly [FinishResult, SessionState] => {
						if (current.closed) {
							return [{ pending: [], should_finish: false }, current];
						}

						return [
							{ pending: [...current.pending.values()], should_finish: true },
							{ ...current, closed: true, pending: new Map() },
						];
					},
				);

				if (!completion.should_finish) {
					return false;
				}

				yield* OfferDiagnostic(diagnostic);

				for (const request of completion.pending) {
					yield* Deferred.fail(request.deferred, failure);
				}

				yield* Queue.end(notification_ingress);
				yield* Queue.end(notifications);
				yield* Queue.end(diagnostics);

				return true;
			});
		const Close = Effect.uninterruptible(
			Effect.gen(function* () {
				const should_close = yield* Ref.modify(state, (current) => {
					if (current.close_requested) {
						return [false, current] as const;
					}

					return [true, { ...current, close_requested: true }] as const;
				});

				if (!should_close) {
					yield* Deferred.await(close_complete);

					return;
				}

				yield* Finish(new CodexAppServerClosedError({ reason: "closed" }), {
					level: "info",
					message: "Codex app-server session closed",
					source: "process",
				});
				yield* handle.Close;
				yield* Deferred.succeed(close_complete, undefined);
			}),
		);
		const NextFrameSequence = Ref.modify(
			state,
			(current) =>
				[
					current.next_frame_sequence + 1,
					{ ...current, next_frame_sequence: current.next_frame_sequence + 1 },
				] as const,
		);
		const DeliverResponse = (
			response: CodexJsonRpcResponseEnvelope,
			frame: CodexJsonlFrame,
			frame_sequence: number,
		) =>
			Effect.gen(function* () {
				const request = yield* RemovePending(response.id);

				if (!request) {
					yield* EmitDiagnostic({
						frame_sequence,
						level: "warning",
						message: `Received an uncorrelated response for request ${String(response.id)}`,
						raw_frame_base64: frame.raw_frame_base64,
						source: "stdout",
					});

					return;
				}

				if ("error" in response) {
					yield* Deferred.fail(
						request.deferred,
						new CodexAppServerResponseError({
							error: response.error,
							id: response.id,
							method: request.method,
						}),
					);

					return;
				}

				yield* Deferred.succeed(request.deferred, response);
			});
		const is_opted_out_bookkeeping_notification = (notification: CodexAppServerNotification) =>
			notification.id === undefined &&
			codex_opt_out_notification_method_set.has(notification.method);
		const EnqueueNotification = (notification: CodexAppServerNotification) =>
			Effect.gen(function* () {
				/**
				 * These exact notification methods were disabled at initialize time and
				 * have no Artisan run projection. A noncompliant older server must not
				 * let them crowd out a completion or an interaction request. Requests
				 * with ids remain lossless even when they reuse a method name.
				 */
				if (is_opted_out_bookkeeping_notification(notification)) {
					return;
				}

				yield* Queue.offer(notification_ingress, notification);
			});
		const ProcessFrame = (frame: CodexJsonlFrame, frame_sequence: number) =>
			Effect.gen(function* () {
				const decoded = yield* DecodeCodexInboundEnvelope(frame.payload).pipe(Effect.exit);

				if (!Exit.isSuccess(decoded)) {
					yield* EmitDiagnostic({
						frame_sequence,
						level: "warning",
						message: "Received an invalid Codex JSON-RPC envelope",
						raw_frame_base64: frame.raw_frame_base64,
						source: "stdout",
					});

					return;
				}

				const envelope = decoded.value;

				if ("method" in envelope) {
					yield* EnqueueNotification({
						frame_sequence,
						...("id" in envelope ? { id: envelope.id } : {}),
						method: envelope.method,
						...(envelope.params === undefined ? {} : { params: envelope.params }),
						payload: frame.payload,
						raw_frame_base64: frame.raw_frame_base64,
					});

					return;
				}

				yield* DeliverResponse(envelope, frame, frame_sequence);
			});
		const ProcessDecode = (decode: CodexJsonlDecode) =>
			Effect.gen(function* () {
				const frame_sequence = yield* NextFrameSequence;

				if (decode instanceof CodexJsonlMalformedLineError) {
					yield* EmitDiagnostic({
						frame_sequence,
						level: "error",
						message: decode.message,
						raw_frame_base64: decode.raw_frame_base64,
						source: "stdout",
					});

					return;
				}

				if (decode instanceof CodexJsonlOversizedLineError) {
					const message = `Codex app-server JSONL frame exceeded ${decode.max_frame_bytes} bytes after receiving ${decode.size_bytes} bytes`;

					yield* EmitDiagnostic({
						frame_sequence,
						level: "error",
						message,
						source: "stdout",
					});

					return yield* Effect.fail(new CodexAppServerProtocolError({ message }));
				}

				yield* ProcessFrame(decode, frame_sequence);
			});
		const framer = new CodexJsonlFramer({ max_frame_bytes });
		const ReadFramedStdout = Stream.fromAsyncIterable(
			handle.Stdout,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				Effect.forEach(framer.PushRecovering(chunk), ProcessDecode),
			),
			Effect.andThen(Effect.forEach(framer.FinishRecovering(), ProcessDecode)),
		);
		const RunStdout = ReadFramedStdout.pipe(
			Effect.flatMap(() => Deferred.succeed(stdout_drained, undefined).pipe(Effect.asVoid)),
			Effect.catchEager((error) =>
				Finish(error, {
					level: "error",
					message:
						error instanceof EngineProcessError
							? `Codex app-server stdout reader failed: ${error.operation}`
							: error.message,
					source: "process",
				}).pipe(
					Effect.andThen(Deferred.fail(stdout_drained, error)),
					Effect.andThen(Close),
				),
			),
		);
		const DeliverNotifications = Stream.fromQueue(notification_ingress).pipe(
			Stream.runForEach((notification) =>
				Queue.offer(notifications, notification).pipe(Effect.asVoid),
			),
			Effect.ensuring(Queue.end(notifications)),
		);
		const DrainStderr = Stream.fromAsyncIterable(
			handle.Stderr,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				EmitDiagnostic({
					level: "info",
					message: new TextDecoder().decode(chunk),
					raw_frame_base64: Buffer.from(chunk).toString("base64"),
					source: "stderr",
				}),
			),
			Effect.catchEager((error) =>
				EmitDiagnostic({
					level: "error",
					message: `Codex app-server stderr reader failed: ${error.operation}`,
					source: "process",
				}),
			),
		);
		const WatchExit = Effect.gen(function* () {
			const exit = yield* handle.Exit;

			yield* Deferred.await(stdout_drained).pipe(Effect.ignore);
			yield* Finish(new CodexAppServerClosedError({ reason: "exited" }), {
				level: "info",
				message: `Codex app-server closed with code ${String(exit.code)} and signal ${String(exit.signal)}`,
				source: "process",
			});
		}).pipe(
			Effect.catchEager((error) =>
				Finish(error, {
					level: "error",
					message: `Codex app-server close watcher failed: ${error.operation}`,
					source: "process",
				}).pipe(Effect.andThen(Close)),
			),
		);

		yield* Effect.forkScoped(DeliverNotifications);
		yield* Effect.forkScoped(RunStdout);
		yield* Effect.forkScoped(DrainStderr);
		yield* Effect.forkScoped(WatchExit);
		yield* Effect.addFinalizer(() => Close.pipe(Effect.ignore));

		const Write = (payload: unknown, operation: SerializationOperation) =>
			Effect.gen(function* () {
				const bytes = yield* EncodeJsonLine(payload, operation);

				yield* Semaphore.withPermit(write_lock)(
					Effect.gen(function* () {
						const current = yield* Ref.get(state);

						if (current.closed) {
							return yield* Effect.fail(
								new CodexAppServerClosedError({ reason: "closed" }),
							);
						}

						yield* handle.Write(bytes);
					}),
				);
			});
		const Request = (method: string, params: unknown, timeout_ms = request_timeout_ms) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					yield* ValidatePositiveOption("timeout_ms", timeout_ms);
					const request = yield* Semaphore.withPermit(write_lock)(
						Effect.gen(function* () {
							const current = yield* Ref.get(state);

							if (current.closed) {
								return yield* Effect.fail(
									new CodexAppServerClosedError({ reason: "closed" }),
								);
							}

							const id = current.next_request_id + 1;
							const bytes = yield* EncodeJsonLine({ id, method, params }, "request");
							const deferred = yield* Deferred.make<
								CodexJsonRpcSuccessEnvelope,
								CodexAppServerSessionFailure
							>();
							const pending = new Map(current.pending);

							pending.set(id, { deferred, method });
							yield* Ref.set(state, {
								...current,
								next_request_id: id,
								pending,
							});

							yield* handle
								.Write(bytes)
								.pipe(
									Effect.catchEager((error) =>
										RemovePending(id).pipe(
											Effect.andThen(Deferred.fail(deferred, error)),
											Effect.andThen(Effect.fail(error)),
										),
									),
								);

							return { deferred, id };
						}),
					);
					const timeout = new CodexAppServerRequestTimeoutError({
						id: request.id,
						method,
						timeout_ms,
					});
					const AwaitResponse = Deferred.await(request.deferred).pipe(
						Effect.timeoutOrElse({
							duration: timeout_ms,
							orElse: () =>
								Effect.gen(function* () {
									const pending = yield* RemovePending(request.id);

									if (!pending) {
										return yield* Deferred.await(request.deferred);
									}

									yield* Deferred.fail(request.deferred, timeout);

									return yield* Effect.fail(timeout);
								}),
						}),
					);

					return yield* restore(AwaitResponse).pipe(
						Effect.onInterrupt(() => RemovePending(request.id).pipe(Effect.asVoid)),
					);
				}),
			);
		const Notify = (method: string, params?: unknown) =>
			Write({ method, ...(params === undefined ? {} : { params }) }, "notify");
		const Respond = (id: CodexRequestId, result: unknown) =>
			Write(make_codex_server_response(id, result), "respond");
		const Handshake = (input: CodexAppServerHandshakeInput) =>
			Effect.gen(function* () {
				const initialize = make_codex_initialize_request(
					input.client_name,
					input.client_version,
				);
				const response = yield* Request(initialize.method, initialize.params);
				const decoded = yield* DecodeCodexInitializeResponse({
					id: response.id,
					result: response.result,
				}).pipe(
					Effect.mapError(
						() =>
							new CodexAppServerProtocolError({
								message: "Codex app-server returned an invalid initialize response",
							}),
					),
				);

				yield* Notify("initialized");

				return decoded;
			});

		return {
			Close,
			Diagnostics: Stream.fromQueue(diagnostics),
			Handshake,
			Notifications: Stream.fromQueue(notifications),
			Notify,
			Request,
			Respond,
		};
	});
};
