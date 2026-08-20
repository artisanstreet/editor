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
	RequestHeld,
	type PendingRequestEnvelope,
	type PendingResultEnvelope,
	type RequestDelivery,
	type SendRequest,
} from "./client-common";

interface PendingRequest {
	/** The caller stopped waiting while a reconnect resend still owned the envelope. */
	readonly abandoned: boolean;
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
	/** A reconnect fiber has atomically claimed this exact registration for resend. */
	readonly retrying: boolean;
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
	| { readonly _tag: "Parked"; readonly error: ArtisanClientError }
	| { readonly _tag: "Registered" };

type PendingMatch =
	| { readonly _tag: "Found"; readonly pending: PendingRequest }
	| { readonly _tag: "Ignored" }
	| { readonly _tag: "Missing" };

export type RequestDeadline = (kind: PendingRequestEnvelope["kind"]) => number;

/**
 * Bounds every control request. Local projection and settings RPCs must answer
 * promptly; operations that intentionally cross a filesystem, process, network,
 * or human boundary receive a larger fail-safe while their UI remains detached.
 */
export const request_deadline_ms_for: RequestDeadline = (kind) => {
	if (kind === "project.directory.pick") {
		return 30 * 60_000;
	}

	/**
	 * A command is the one request whose deadline is not a fail-safe.
	 *
	 * Every other kind here is a read: abandoning one costs a retry. Abandoning a
	 * command decides nothing — Forge may still accept it — so the caller is left
	 * holding a message it cannot say was sent, which is how a send that landed
	 * came back as "did not answer before its deadline" and stayed invisible
	 * until a reload. It carries attachment bytes and a durable journal write, so
	 * it is given room to finish rather than the 2s a local projection read gets.
	 * A dead connection still fails it immediately, through parking rather than
	 * through this.
	 */
	/**
	 * Thread creation is the same kind of thing, and losing it is worse.
	 *
	 * It was on the 2s read default, which is a deadline for the *reply* rather
	 * than for the work: Forge accepts a `thread.create` in 2ms at the median and
	 * 233ms at its worst, so every failure here is the answer being late, never
	 * the thread being unmade. Abandoning it therefore leaves a thread that
	 * exists, is listed, and is titled "New thread" — while the caller drops the
	 * first message it was created to carry, with nothing to retry from and
	 * nothing on screen to say so.
	 *
	 * That the reply can be that late is not hypothetical: commands on this same
	 * socket record 17.5s and 20.3s round trips in this database. A read that
	 * misses its deadline costs a retry; this one costs the message.
	 */
	if (kind === "command" || kind === "thread.create.request") {
		return 30_000;
	}

	if (kind === "engine.authentication.request") {
		return 10 * 60_000;
	}

	/**
	 * A machine connect may cold-boot a WSL distribution and start a fresh
	 * Forge before the handoff prints. The backend bounds itself at two
	 * minutes; staying above that budget keeps its bounded failure the one
	 * that surfaces.
	 */
	if (kind === "host.machines.connect.request") {
		return 150_000;
	}

	/** Enumerating machines may pay a one-time WSL service spin-up. */
	if (kind === "host.machines.query") {
		return 15_000;
	}

	/**
	 * Provider-boundary reads. The backend spawns or probes an external CLI to
	 * answer these and bounds itself at 15 seconds — a usage lookup runs the
	 * provider's own `auth`/`usage` commands, and a cold behaviour read runs the
	 * Codex probe.
	 *
	 * A deadline shorter than the server's own budget cannot report anything
	 * true: it abandons a request that is still legitimately running and calls
	 * it a deadline miss. On the 2s local-read default that made every Claude
	 * usage row read as a timeout while Forge was still asking the provider.
	 * Kept above the server's budget so the server's bounded, meaningful
	 * failure is what surfaces instead of a guess made here.
	 */
	if (kind === "engine.usage.query" || kind === "model_behaviour.query") {
		return 20_000;
	}

	if (
		kind === "engine.install.request" ||
		kind === "engine.rollback.request" ||
		kind === "marketplace.capability.connect.request" ||
		kind === "marketplace.capability.invoke.request" ||
		kind === "marketplace.capability.oauth.begin" ||
		kind === "marketplace.capability.oauth.complete" ||
		kind === "marketplace.capability.oauth.refresh" ||
		kind === "marketplace.npx_skills.discover" ||
		kind === "marketplace.npx_skills.import.request" ||
		kind === "marketplace.routine.install.request" ||
		kind === "marketplace.routine.invoke"
	) {
		return 2 * 60_000;
	}

	/**
	 * The thread-route gate blocks navigation on this read. On a cold or
	 * reconnecting session it is issued before the transport is ready, and the
	 * 2s local-read default expired while the request was still legitimately
	 * held — which surfaced as a failed handoff into a thread that had just
	 * been created.
	 */
	if (kind === "thread.open.query") {
		return 15_000;
	}

	if (
		kind.startsWith("git.") ||
		kind.startsWith("workspace.") ||
		kind === "message.image_attachment.query" ||
		kind === "preview.asset.metadata.query" ||
		kind === "preview.browser.launch" ||
		kind === "preview.rich_link.resolve.query" ||
		kind === "preview.target.probe" ||
		kind === "project.diff.query" ||
		kind === "project.directory.create" ||
		/**
		 * Listing crosses the filesystem exactly like create/select: a large
		 * home directory with cloud placeholders can honestly take longer than
		 * the local-read default, and the abandoned answer made directory rows
		 * silently do nothing.
		 */
		kind === "project.directory.list.query" ||
		kind === "project.directory.select" ||
		kind === "project.repository.query"
	) {
		return 15_000;
	}

	return 2_000;
};

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
	send_request: SendRequest,
	request_deadline: RequestDeadline = request_deadline_ms_for,
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

				if (found.retrying) {
					pending.set(request.envelope.message_id, { ...found, abandoned: true });
					return { ...current, pending };
				}

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
					abandoned: false,
					deferred,
					delivery: RequestDelivered,
					envelope,
					retrying: false,
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
							Effect.timeoutOrElse({
								duration: request_deadline(envelope.kind),
								orElse: () =>
									Effect.fail(
										client_error(
											"connection",
											`Artisan Forge did not answer the ${envelope.kind} request before its deadline.`,
											new Error("request deadline exceeded"),
											true,
											"request.timeout",
										),
									),
							}),
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
					const ignored_correlations = new Set(current.ignored_correlations);

					next.delete(envelope.correlation_id);
					if (pending.retrying) ignored_correlations.add(envelope.correlation_id);

					return [
						{ _tag: "Found", pending },
						{ ...current, ignored_correlations, pending: next },
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
					const ignored_correlations = new Set(current.ignored_correlations);

					next.delete(correlation_id);
					if (found.retrying) ignored_correlations.add(correlation_id);

					return [
						{ _tag: "Found", pending: found },
						{ ...current, ignored_correlations, pending: next },
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
			pending: new Map(
				[...current.pending].map(([correlation_id, pending]) => [
					correlation_id,
					pending.retrying ? pending : { ...pending, delivery: RequestHeld },
				]),
			),
		}));

		/** Claims an exact still-pending registration before carrying it again. */
		const claim_retry = (request: PendingRequest) =>
			Ref.modify(state, (current) => {
				const found = held_by(current, request);
				if (found === undefined || found.retrying) return [false, current] as const;

				return [
					true,
					{
						...current,
						pending: new Map(current.pending).set(request.envelope.message_id, {
							...found,
							/** Pessimistic until send reports otherwise. */
							delivery: RequestDelivered,
							retrying: true,
						}),
					},
				] as const;
			});

		/**
		 * Settles resend ownership even when the resend fiber is interrupted. An
		 * abandoned caller is forgotten only when no session carried the retry;
		 * otherwise its correlation remains as a one-result tombstone.
		 */
		const finish_retry = (request: PendingRequest, delivery: RequestDelivery) =>
			Ref.update(state, (current) => {
				const found = held_by(current, request);
				if (found === undefined || !found.retrying) return current;

				const pending = new Map(current.pending);
				if (!found.abandoned) {
					pending.set(request.envelope.message_id, {
						...found,
						delivery,
						retrying: false,
					});
					return { ...current, pending };
				}

				pending.delete(request.envelope.message_id);
				if (delivery._tag === "Held") return { ...current, pending };

				const ignored_correlations = new Set(current.ignored_correlations);
				ignored_correlations.add(request.envelope.message_id);
				return { ...current, ignored_correlations, pending };
			});

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
						Effect.gen(function* () {
							if (!(yield* claim_retry(pending))) return;
							yield* send_request(pending.envelope).pipe(
								Effect.onExit((exit) =>
									finish_retry(
										pending,
										exit._tag === "Success" ? exit.value : RequestHeld,
									),
								),
							);
						}),
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
