import { Deferred, Effect, Ref, Schema } from "effect";

import {
	GetControlRpc,
	type ControlRpcSuccessFor,
	type ProtocolErrorDetail,
} from "@artisan/protocol";

import type { ArtisanClientError } from "../client-api/service";
import {
	client_error,
	protocol_client_error,
	RequestDelivered,
	type PendingRequestEnvelope,
	type PendingResultEnvelope,
	type RequestDelivery,
	type SendRequest,
} from "./client-common";

interface PendingRequest {
	readonly deferred: Deferred.Deferred<PendingResultEnvelope, ArtisanClientError>;
	/**
	 * Whether a session may already hold this envelope, and so whether the
	 * backend still owes an answer to it. It starts pessimistic so a caller
	 * interrupted mid-flight never forgets a correlation the wire has, and is
	 * corrected whenever a send reports which of the two actually happened.
	 */
	readonly delivery: RequestDelivery;
	readonly envelope: PendingRequestEnvelope;
	readonly accepts: (envelope: PendingResultEnvelope) => boolean;
}

interface RequestState {
	readonly disposed: boolean;
	readonly ignored_correlations: ReadonlySet<string>;
	/**
	 * Set once the connection has spent its reconnect budget. A request holds
	 * only for as long as something will carry it: while a session is merely
	 * missing it waits to be retried on reconnect, but a parked connection has
	 * nothing left to retry with, so waiting becomes waiting forever.
	 */
	readonly parked: ArtisanClientError | undefined;
	readonly pending: ReadonlyMap<string, PendingRequest>;
}

type RequestRegistration =
	| { readonly _tag: "Conflict" }
	| { readonly _tag: "Disposed" }
	| { readonly _tag: "Overflow" }
	| { readonly _tag: "Parked"; readonly error: ArtisanClientError }
	| { readonly _tag: "Registered" };

type PendingMatch =
	| { readonly _tag: "Found"; readonly pending: PendingRequest }
	| { readonly _tag: "Ignored" }
	| { readonly _tag: "Missing" };

/** Owns exact request envelopes until one durable or correlated result completes. */
export interface ClientRequestCoordinator {
	readonly Dispose: (error: ArtisanClientError) => Effect.Effect<void>;
	/** Fails everything in flight once the connection stops trying to reconnect. */
	readonly Park: (error: ArtisanClientError) => Effect.Effect<void>;
	readonly Reject: (
		correlation_id: string,
		detail: ProtocolErrorDetail,
	) => Effect.Effect<boolean, ArtisanClientError>;
	readonly Request: <Request extends PendingRequestEnvelope>(
		envelope: Request,
	) => Effect.Effect<ControlRpcSuccessFor<Request>, ArtisanClientError>;
	readonly ResetConnection: Effect.Effect<void>;
	readonly Resolve: (envelope: PendingResultEnvelope) => Effect.Effect<void, ArtisanClientError>;
	/** Lifts the parked state when a session carries requests again. */
	readonly Resume: Effect.Effect<void>;
	readonly Retry: Effect.Effect<void, ArtisanClientError>;
}

/** Builds the exact-envelope retry and correlation coordinator. */
export const make_client_request_coordinator = (
	max_pending_requests: number,
	send_request: SendRequest,
) =>
	Effect.gen(function* () {
		const state = yield* Ref.make<RequestState>({
			disposed: false,
			ignored_correlations: new Set(),
			parked: undefined,
			pending: new Map(),
		});

		const register = (request: PendingRequest) =>
			Ref.modify<RequestState, RequestRegistration>(state, (current) => {
				if (current.disposed) {
					return [{ _tag: "Disposed" }, current];
				}

				if (current.parked !== undefined) {
					return [{ _tag: "Parked", error: current.parked }, current];
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

		/**
		 * The registration this caller still owns, if it owns one. A request
		 * already resolved, rejected, parked, or disposed has handed its slot
		 * back, so a late interrupt must neither evict whatever holds it now nor
		 * resurrect a correlation that has already been accounted for.
		 */
		const held_by = (current: RequestState, request: PendingRequest) => {
			const found = current.pending.get(request.envelope.message_id);

			return found?.deferred === request.deferred ? found : undefined;
		};

		/** Records what a send turned out to be, for this caller's registration only. */
		const record_delivery = (request: PendingRequest, delivery: RequestDelivery) =>
			Ref.update(state, (current) => {
				const found = held_by(current, request);

				if (found === undefined) {
					return current;
				}

				return {
					...current,
					pending: new Map(current.pending).set(request.envelope.message_id, {
						...found,
						delivery,
					}),
				};
			});

		/** Drops a request nothing will answer, leaving its id free to be reused. */
		const forget = (request: PendingRequest) =>
			Ref.update(state, (current) => {
				if (held_by(current, request) === undefined) {
					return current;
				}

				const pending = new Map(current.pending);

				pending.delete(request.envelope.message_id);

				return { ...current, pending };
			});

		/**
		 * Releases the slot a caller claimed once that caller stops waiting for
		 * its result. A delivered envelope leaves its correlation behind so the
		 * answer the backend still owes is discarded rather than mistaken for a
		 * later caller's; an envelope no session ever carried is forgotten
		 * outright, because nothing will ever answer it and remembering it would
		 * spend capacity guarding against a result that cannot arrive.
		 */
		const abandon = (request: PendingRequest) =>
			Ref.update(state, (current) => {
				const found = held_by(current, request);

				if (found === undefined) {
					return current;
				}

				const pending = new Map(current.pending);

				pending.delete(request.envelope.message_id);

				if (found.delivery._tag === "Held") {
					return { ...current, pending };
				}

				const ignored_correlations = new Set(current.ignored_correlations);

				ignored_correlations.add(request.envelope.message_id);

				return { ...current, ignored_correlations, pending };
			});

		const request = <Request extends PendingRequestEnvelope>(envelope: Request) =>
			Effect.gen(function* () {
				const deferred = yield* Deferred.make<PendingResultEnvelope, ArtisanClientError>();
				const rpc = GetControlRpc(envelope.kind);
				const pending = {
					accepts: Schema.is(rpc.successSchema),
					deferred,
					delivery: RequestDelivered,
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
					case "Parked":
						return yield* Effect.fail(registration.error);
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
						/**
						 * The send and the wait are one interruptible region: a
						 * registration that outlives its caller unreleased is a
						 * slot nothing can ever reclaim. A failed send is the one
						 * exit that forgets outright — it tears its session down
						 * with the envelope, so no answer can follow it.
						 */
						return (yield* send_request(envelope).pipe(
							Effect.tapError(() => forget(pending)),
							Effect.onInterrupt(() => abandon(pending)),
							Effect.tap((delivery) => record_delivery(pending, delivery)),
							Effect.andThen(
								Deferred.await(deferred).pipe(
									Effect.onInterrupt(() => abandon(pending)),
								),
							),
						)) as ControlRpcSuccessFor<Request>;
				}
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

		/**
		 * A held request becomes an answered one here, so each resend reports
		 * back: a caller that walks away afterwards must remember the
		 * correlation this session finally carried for it.
		 */
		const retry = Ref.get(state).pipe(
			Effect.flatMap((current) =>
				Effect.forEach(
					current.pending.values(),
					(pending) =>
						send_request(pending.envelope).pipe(
							Effect.flatMap((delivery) => record_delivery(pending, delivery)),
						),
					{ discard: true },
				),
			),
		);

		/**
		 * The session that owed those answers is gone and no later one will
		 * deliver them, so the correlations abandoned under it are dropped
		 * alongside the requests still waiting: holding either would spend
		 * capacity guarding against results that can no longer arrive.
		 */
		const park = (error: ArtisanClientError) =>
			Effect.gen(function* () {
				const current = yield* Ref.getAndUpdate(state, (value) => ({
					...value,
					ignored_correlations: new Set<string>(),
					parked: error,
					pending: new Map<string, PendingRequest>(),
				}));

				yield* Effect.forEach(
					current.pending.values(),
					(pending) => Deferred.fail(pending.deferred, error),
					{ discard: true },
				);
			});

		const resume = Ref.update(state, (current) => ({ ...current, parked: undefined }));

		const dispose = (error: ArtisanClientError) =>
			Effect.gen(function* () {
				const current = yield* Ref.getAndSet(state, {
					disposed: true,
					ignored_correlations: new Set<string>(),
					parked: undefined,
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
			Park: park,
			Reject: reject,
			Request: request,
			ResetConnection: reset_connection,
			Resolve: resolve,
			Resume: resume,
			Retry: retry,
		} satisfies ClientRequestCoordinator;
	});
