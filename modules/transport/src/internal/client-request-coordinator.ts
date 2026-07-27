import { Deferred, Effect, Ref, Schema } from "effect";

import {
	GetControlRpc,
	type ControlRpcSuccessFor,
	type ProtocolErrorDetail,
} from "@artisan/protocol";

import type { ArtisanClientError } from "../client-contract";
import {
	client_error,
	protocol_client_error,
	type PendingRequestEnvelope,
	type PendingResultEnvelope,
	type SendCurrent,
} from "./client-common";

interface PendingRequest {
	readonly deferred: Deferred.Deferred<PendingResultEnvelope, ArtisanClientError>;
	readonly envelope: PendingRequestEnvelope;
	readonly accepts: (envelope: PendingResultEnvelope) => boolean;
}

interface RequestState {
	readonly disposed: boolean;
	readonly ignored_correlations: ReadonlySet<string>;
	readonly pending: ReadonlyMap<string, PendingRequest>;
}

type RequestRegistration =
	| { readonly _tag: "Conflict" }
	| { readonly _tag: "Disposed" }
	| { readonly _tag: "Overflow" }
	| { readonly _tag: "Registered" };

type PendingMatch =
	| { readonly _tag: "Found"; readonly pending: PendingRequest }
	| { readonly _tag: "Ignored" }
	| { readonly _tag: "Missing" };

/** Owns exact request envelopes until one durable or correlated result completes. */
export interface ClientRequestCoordinator {
	readonly Dispose: (error: ArtisanClientError) => Effect.Effect<void>;
	readonly Reject: (
		correlation_id: string,
		detail: ProtocolErrorDetail,
	) => Effect.Effect<boolean, ArtisanClientError>;
	readonly Request: <Request extends PendingRequestEnvelope>(
		envelope: Request,
	) => Effect.Effect<ControlRpcSuccessFor<Request>, ArtisanClientError>;
	readonly ResetConnection: Effect.Effect<void>;
	readonly Resolve: (envelope: PendingResultEnvelope) => Effect.Effect<void, ArtisanClientError>;
	readonly Retry: Effect.Effect<void, ArtisanClientError>;
}

/** Builds the exact-envelope retry and correlation coordinator. */
export const make_client_request_coordinator = (
	max_pending_requests: number,
	send_current: SendCurrent,
) =>
	Effect.gen(function* () {
		const state = yield* Ref.make<RequestState>({
			disposed: false,
			ignored_correlations: new Set(),
			pending: new Map(),
		});

		const register = (request: PendingRequest) =>
			Ref.modify<RequestState, RequestRegistration>(state, (current) => {
				if (current.disposed) {
					return [{ _tag: "Disposed" }, current];
				}

				if (
					current.pending.has(request.envelope.message_id) ||
					current.ignored_correlations.has(request.envelope.message_id)
				) {
					return [{ _tag: "Conflict" }, current];
				}

				if (
					current.pending.size + current.ignored_correlations.size >=
					max_pending_requests
				) {
					return [{ _tag: "Overflow" }, current];
				}

				return [
					{ _tag: "Registered" },
					{
						...current,
						pending: new Map(current.pending).set(request.envelope.message_id, request),
					},
				];
			});
		const unregister_failed_send = (request: PendingRequest) =>
			Ref.update(state, (current) => {
				if (current.pending.get(request.envelope.message_id) !== request) {
					return current;
				}

				const pending = new Map(current.pending);

				pending.delete(request.envelope.message_id);

				return { ...current, pending };
			});

		const request = <Request extends PendingRequestEnvelope>(envelope: Request) =>
			Effect.gen(function* () {
				const deferred = yield* Deferred.make<PendingResultEnvelope, ArtisanClientError>();
				const rpc = GetControlRpc(envelope.kind);
				const pending = {
					accepts: Schema.is(rpc.successSchema),
					deferred,
					envelope,
				};
				const registration = yield* register(pending);

				switch (registration._tag) {
					case "Disposed":
						return yield* Effect.fail(
							client_error(
								"disposed",
								"The Artisan client was disposed.",
								new Error("client disposed"),
							),
						);
					case "Conflict":
						return yield* Effect.fail(
							client_error(
								"correlation_conflict",
								"A request id is already active.",
								new Error("request id collision"),
							),
						);
					case "Overflow":
						return yield* Effect.fail(
							client_error(
								"request_overflow",
								"The pending request limit was reached.",
								new Error("pending request capacity reached"),
							),
						);
					case "Registered":
						yield* send_current(envelope).pipe(
							Effect.tapError(() => unregister_failed_send(pending)),
						);
				}

				return (yield* Deferred.await(deferred).pipe(
					Effect.onInterrupt(() =>
						Ref.update(state, (current) => {
							const pending_requests = new Map(current.pending);
							const ignored_correlations = new Set(current.ignored_correlations);

							pending_requests.delete(envelope.message_id);
							ignored_correlations.add(envelope.message_id);

							return {
								...current,
								ignored_correlations,
								pending: pending_requests,
							};
						}),
					),
				)) as ControlRpcSuccessFor<Request>;
			});

		const resolve = (envelope: PendingResultEnvelope) =>
			Effect.gen(function* () {
				const match = yield* Ref.modify<RequestState, PendingMatch>(state, (current) => {
					const pending = current.pending.get(envelope.correlation_id);

					if (!pending) {
						if (current.ignored_correlations.has(envelope.correlation_id)) {
							const ignored_correlations = new Set(current.ignored_correlations);

							ignored_correlations.delete(envelope.correlation_id);

							return [{ _tag: "Ignored" }, { ...current, ignored_correlations }];
						}

						return [{ _tag: "Missing" }, current];
					}

					const next = new Map(current.pending);

					next.delete(envelope.correlation_id);

					return [
						{ _tag: "Found", pending },
						{ ...current, pending: next },
					];
				});

				if (match._tag === "Ignored") {
					return;
				}

				if (match._tag === "Missing") {
					return yield* Effect.fail(
						client_error(
							"correlation_conflict",
							"The backend returned an unknown correlation id.",
							new Error("unknown response correlation"),
						),
					);
				}

				if (!match.pending.accepts(envelope)) {
					const error = client_error(
						"correlation_conflict",
						"The backend response kind did not match its request.",
						new Error("response kind mismatch"),
					);

					yield* Deferred.fail(match.pending.deferred, error);

					return yield* Effect.fail(error);
				}

				yield* Deferred.succeed(match.pending.deferred, envelope);
			});

		const reject = (correlation_id: string, detail: ProtocolErrorDetail) =>
			Effect.gen(function* () {
				const match = yield* Ref.modify<RequestState, PendingMatch>(state, (current) => {
					const found = current.pending.get(correlation_id);

					if (!found) {
						if (current.ignored_correlations.has(correlation_id)) {
							const ignored_correlations = new Set(current.ignored_correlations);

							ignored_correlations.delete(correlation_id);

							return [{ _tag: "Ignored" }, { ...current, ignored_correlations }];
						}

						return [{ _tag: "Missing" }, current];
					}

					const next = new Map(current.pending);

					next.delete(correlation_id);

					return [
						{ _tag: "Found", pending: found },
						{ ...current, pending: next },
					];
				});

				if (match._tag === "Missing") {
					return false;
				}

				if (match._tag === "Found") {
					yield* Deferred.fail(match.pending.deferred, protocol_client_error(detail));
				}

				return true;
			});
		const reset_connection = Ref.update(state, (current) => ({
			...current,
			ignored_correlations: new Set<string>(),
		}));

		const retry = Ref.get(state).pipe(
			Effect.flatMap((current) =>
				Effect.forEach(
					current.pending.values(),
					(pending) => send_current(pending.envelope),
					{ discard: true },
				),
			),
		);

		const dispose = (error: ArtisanClientError) =>
			Effect.gen(function* () {
				const current = yield* Ref.getAndSet(state, {
					disposed: true,
					ignored_correlations: new Set<string>(),
					pending: new Map(),
				});

				yield* Effect.forEach(
					current.pending.values(),
					(pending) => Deferred.fail(pending.deferred, error),
					{ discard: true },
				);
			});

		return {
			Dispose: dispose,
			Reject: reject,
			Request: request,
			ResetConnection: reset_connection,
			Resolve: resolve,
			Retry: retry,
		} satisfies ClientRequestCoordinator;
	});
