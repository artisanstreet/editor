import {
	Cause,
	Deferred,
	Effect,
	Exit,
	Fiber,
	Layer,
	Option,
	PubSub,
	Ref,
	Result,
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	CommandEnvelope,
	PreviewBrowserLifecycleQuery,
	type CommandEnvelope as Command,
} from "@artisan/protocol";

import {
	BrowserInspectionConnector,
	ExternalUrlLauncher,
	PreviewBrowserLifecycle,
	PreviewBrowserLifecycleError,
	type BrowserInspectionSession,
	type PreviewBrowserAcceptance,
	type PreviewBrowserOperationClaim,
	type PreviewInspectionRevocation,
	type PreviewTargetRemovalClaim,
	type PreviewTargetRemovalSettlement,
} from "./preview-browser";
import {
	PreviewBrowserRepository,
	PreviewBrowserRepositoryUnavailable,
	map_preview_browser_repository_error,
	type PreviewBrowserLaunchPreparation,
	type PreviewBrowserLaunchSettlement,
	type PreviewBrowserRepositoryError,
	type PreviewInspectionAttachSettlement,
	type PreviewInspectionPreparation,
} from "./preview-browser-repository";
import { PreviewTargetClock } from "./preview-target";

type LiveCleanupReason = "connection_lost" | "interrupted" | "target_changed" | "thread_erased";

interface LiveCleanupResult {
	readonly claim: PreviewBrowserOperationClaim;
	readonly complete: boolean;
}

interface LiveInspection {
	readonly claim: Ref.Ref<PreviewBrowserOperationClaim>;
	readonly cleanup_done: Deferred.Deferred<void>;
	readonly cleanup_reason: Ref.Ref<LiveCleanupReason | null>;
	readonly cleanup_started: Ref.Ref<boolean>;
	readonly connector_id: string;
	readonly lock: Semaphore.Semaphore;
	readonly maintenance_scope: Scope.Closeable;
	readonly project_id: string;
	readonly scope: Scope.Closeable;
	readonly session: BrowserInspectionSession;
	readonly settle_cleanup: (
		reason: LiveCleanupReason,
		claim: PreviewBrowserOperationClaim,
	) => Effect.Effect<void>;
	readonly shutdown: Deferred.Deferred<void>;
	readonly target_id: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

type ReadyLaunchPreparation = Exclude<
	PreviewBrowserLaunchPreparation,
	{ readonly _tag: "Pending" }
>;
type ReadyInspectionPreparation = Exclude<
	PreviewInspectionPreparation,
	{ readonly _tag: "Pending" }
>;

/** Configures bounded private coordination without exposing implementation dials to users. */
export interface PreviewBrowserLifecycleOptions {
	readonly connector_timeout_ms?: number;
	readonly inspection_heartbeat_interval_ms?: number;
	readonly launcher_timeout_ms?: number;
	readonly live_inspection_lease_ms?: number;
	readonly operation_lease_ms?: number;
	readonly operation_poll_interval_ms?: number;
	readonly recovery_interval_ms?: number;
	readonly sliding_event_capacity?: number;
	readonly target_removal_lease_ms?: number;
	readonly teardown_timeout_ms?: number;
}

function lifecycle_error(subject_id: string, code: PreviewBrowserLifecycleError["code"]) {
	return new PreviewBrowserLifecycleError({ code, subject_id });
}

function is_launch_command(command: Command): command is Command & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.browser.open" }>;
} {
	return command.payload.type === "preview.browser.open";
}

function is_attach_command(command: Command): command is Command & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.inspection.attach" }>;
} {
	return command.payload.type === "preview.inspection.attach";
}

function is_detach_command(command: Command): command is Command & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.inspection.detach" }>;
} {
	return command.payload.type === "preview.inspection.detach";
}

function typed_failure<A, E>(exit: Exit.Exit<A, E>): E | undefined {
	if (Exit.isSuccess(exit)) {
		return undefined;
	}

	const found = Cause.findError(exit.cause);

	return Result.isFailure(found) ? undefined : found.success;
}

function validate_inspection_session(value: unknown): BrowserInspectionSession | undefined {
	try {
		if (typeof value !== "object" || value === null) {
			return undefined;
		}

		const detach = Reflect.get(value, "Detach");
		const disconnected = Reflect.get(value, "Disconnected");

		if (!Effect.isEffect(detach) || !Effect.isEffect(disconnected)) {
			return undefined;
		}

		return {
			Detach: detach as BrowserInspectionSession["Detach"],
			Disconnected: disconnected as BrowserInspectionSession["Disconnected"],
		};
	} catch {
		return undefined;
	}
}

function acceptance_is_owned_attachment(
	acceptance: PreviewBrowserAcceptance,
	inspection_id: string,
) {
	return (
		acceptance.status === "accepted" &&
		acceptance.event.payload.type === "preview.inspection.updated" &&
		acceptance.event.payload.inspection.inspection_id === inspection_id &&
		acceptance.event.payload.inspection.state === "attached"
	);
}

/** Builds shell-neutral browser lifecycle coordination around replaceable Effect adapters. */
export function make_preview_browser_lifecycle_layer(options: PreviewBrowserLifecycleOptions = {}) {
	const connector_timeout_ms = options.connector_timeout_ms ?? 15_000;
	const inspection_heartbeat_interval_ms = options.inspection_heartbeat_interval_ms ?? 5_000;
	const launcher_timeout_ms = options.launcher_timeout_ms ?? 10_000;
	const live_inspection_lease_ms = options.live_inspection_lease_ms ?? 30_000;
	const operation_lease_ms = options.operation_lease_ms ?? 30_000;
	const operation_poll_interval_ms = options.operation_poll_interval_ms ?? 25;
	const recovery_interval_ms = options.recovery_interval_ms ?? 1_000;
	const sliding_event_capacity = options.sliding_event_capacity ?? 128;
	const target_removal_lease_ms = options.target_removal_lease_ms ?? 60_000;
	const target_removal_heartbeat_interval_ms = Math.max(
		1,
		Math.floor(target_removal_lease_ms / 3),
	);
	const teardown_timeout_ms = options.teardown_timeout_ms ?? 1_000;

	return Layer.effect(
		PreviewBrowserLifecycle,
		Effect.gen(function* () {
			if (
				!Number.isSafeInteger(connector_timeout_ms) ||
				connector_timeout_ms <= 0 ||
				connector_timeout_ms >= operation_lease_ms ||
				!Number.isSafeInteger(inspection_heartbeat_interval_ms) ||
				inspection_heartbeat_interval_ms <= 0 ||
				inspection_heartbeat_interval_ms >= live_inspection_lease_ms ||
				!Number.isSafeInteger(launcher_timeout_ms) ||
				launcher_timeout_ms <= 0 ||
				launcher_timeout_ms >= operation_lease_ms ||
				!Number.isSafeInteger(live_inspection_lease_ms) ||
				live_inspection_lease_ms <= 0 ||
				live_inspection_lease_ms <=
					inspection_heartbeat_interval_ms + teardown_timeout_ms ||
				live_inspection_lease_ms > 600_000 ||
				!Number.isSafeInteger(operation_lease_ms) ||
				operation_lease_ms <= 0 ||
				operation_lease_ms > 600_000 ||
				!Number.isSafeInteger(operation_poll_interval_ms) ||
				operation_poll_interval_ms <= 0 ||
				operation_poll_interval_ms >= operation_lease_ms ||
				!Number.isSafeInteger(recovery_interval_ms) ||
				recovery_interval_ms <= 0 ||
				recovery_interval_ms > 60_000 ||
				!Number.isSafeInteger(sliding_event_capacity) ||
				sliding_event_capacity <= 0 ||
				!Number.isSafeInteger(target_removal_lease_ms) ||
				target_removal_lease_ms <= teardown_timeout_ms ||
				target_removal_lease_ms > 600_000 ||
				!Number.isSafeInteger(teardown_timeout_ms) ||
				teardown_timeout_ms <= 0 ||
				teardown_timeout_ms > 60_000
			) {
				return yield* lifecycle_error("", "invalid_request");
			}

			const clock = yield* PreviewTargetClock;
			const connector = yield* BrowserInspectionConnector;
			const launcher = yield* ExternalUrlLauncher;
			const repository = yield* PreviewBrowserRepository;
			const events = yield* Effect.acquireRelease(
				PubSub.sliding<PreviewBrowserAcceptance["event"]>(sliding_event_capacity),
				PubSub.shutdown,
			);
			const service_scope = yield* Scope.make();
			const live_inspections = yield* Ref.make(new Map<string, LiveInspection>());
			const command_lock = yield* Semaphore.make(1);

			const PublishAcceptance = (acceptance: PreviewBrowserAcceptance) =>
				acceptance.status === "accepted"
					? PubSub.publish(events, acceptance.event).pipe(Effect.asVoid)
					: Effect.void;
			const PublishEvent = (event: PreviewBrowserAcceptance["event"]) =>
				PubSub.publish(events, event).pipe(Effect.asVoid);
			const MapRepositoryError = (subject_id: string) =>
				Effect.mapError((error: PreviewBrowserRepositoryError) =>
					map_preview_browser_repository_error(error, subject_id),
				);
			const DecodeCommand = (command: Command) =>
				Schema.decodeUnknownEffect(CommandEnvelope, { onExcessProperty: "error" })(
					command,
				).pipe(
					Effect.mapError(() =>
						lifecycle_error(command.message_id ?? "", "invalid_request"),
					),
				);
			const InterruptDetached = <A, E>(fiber: Fiber.Fiber<A, E>) =>
				Fiber.interrupt(fiber).pipe(
					Effect.forkDetach({ startImmediately: true }),
					Effect.asVoid,
				);
			const RunBoundedDetached = <A, E, R>(effect: Effect.Effect<A, E, R>) =>
				Effect.gen(function* () {
					const fiber = yield* effect.pipe(Effect.forkDetach({ startImmediately: true }));
					const awaited = yield* Fiber.await(fiber).pipe(
						Effect.timeoutOption(teardown_timeout_ms),
						Effect.onInterrupt(() => InterruptDetached(fiber)),
					);

					if (Option.isSome(awaited)) {
						return awaited;
					}

					yield* InterruptDetached(fiber);

					return Option.none<Exit.Exit<A, E>>();
				});
			const RunAdapter = <A, E, R>(effect: Effect.Effect<A, E, R>, timeout_ms: number) =>
				Effect.gen(function* () {
					const fiber = yield* effect.pipe(Effect.forkDetach({ startImmediately: true }));
					const awaited = yield* Fiber.await(fiber).pipe(
						Effect.timeoutOption(timeout_ms),
						Effect.onInterrupt(() => InterruptDetached(fiber)),
					);

					if (Option.isSome(awaited)) {
						return awaited;
					}

					yield* InterruptDetached(fiber);

					return Option.none<Exit.Exit<A, E>>();
				});
			const RevokeInspection = (revocation: PreviewInspectionRevocation) =>
				RunAdapter(connector.Revoke(revocation), connector_timeout_ms).pipe(
					Effect.flatMap((revoked) =>
						Option.isSome(revoked) && Exit.isSuccess(revoked.value)
							? Effect.void
							: Effect.fail(lifecycle_error(revocation.inspection_id, "unavailable")),
					),
				);
			const RevokeStoredInspection = (inspection_id: string) =>
				repository.InspectionRevocation(inspection_id).pipe(
					MapRepositoryError(inspection_id),
					Effect.flatMap(
						Option.match({
							onNone: () => Effect.succeed(false),
							onSome: (revocation) =>
								RevokeInspection(revocation).pipe(Effect.as(true)),
						}),
					),
				);
			const CloseScope = (scope: Scope.Closeable) =>
				RunBoundedDetached(Scope.close(scope, Exit.void)).pipe(Effect.asVoid);
			const RemoveLive = (inspection_id: string, live: LiveInspection) =>
				Ref.modify(live_inspections, (current) => {
					if (current.get(inspection_id) !== live) {
						return [false, current];
					}

					const next = new Map(current);

					next.delete(inspection_id);

					return [true, next];
				}).pipe(
					Effect.flatMap((removed) =>
						removed ? Deferred.succeed(live.shutdown, undefined) : Effect.void,
					),
					Effect.asVoid,
				);
			const MaintainCleanupLease = (
				inspection_id: string,
				live: LiveInspection,
			): Effect.Effect<void> =>
				Effect.gen(function* () {
					const claim = yield* Ref.get(live.claim);
					const now_ms = yield* clock.Now;
					const renewed = yield* repository
						.RenewInspectionCleanupLease(
							inspection_id,
							claim,
							now_ms,
							live_inspection_lease_ms,
						)
						.pipe(Effect.exit);

					if (Exit.isSuccess(renewed)) {
						yield* Ref.set(live.claim, renewed.value);
					} else {
						const failure = typed_failure(renewed);

						if (
							failure instanceof PreviewBrowserRepositoryUnavailable &&
							failure.reason === "ownership_lost"
						) {
							return;
						}
					}

					yield* Effect.sleep(inspection_heartbeat_interval_ms);
					return yield* Effect.suspend(() => MaintainCleanupLease(inspection_id, live));
				});
			const PerformLiveCleanup = (inspection_id: string, live: LiveInspection) =>
				Effect.gen(function* () {
					const authority_revoked = yield* Deferred.make<"connector" | "scope">();
					const detach_fiber = yield* Effect.exit(live.session.Detach).pipe(
						Effect.forkDetach({ startImmediately: true }),
					);
					const maintenance_fiber = yield* Effect.exit(
						Scope.close(live.maintenance_scope, Exit.void),
					).pipe(Effect.forkDetach({ startImmediately: true }));
					const lease_fiber = yield* MaintainCleanupLease(inspection_id, live).pipe(
						Effect.forkDetach({ startImmediately: true }),
					);
					const scope_fiber = yield* Scope.close(live.scope, Exit.void).pipe(
						Effect.andThen(Deferred.succeed(authority_revoked, "scope")),
						Effect.catchCause(() => Effect.void),
						Effect.forkDetach({ startImmediately: true }),
					);
					const revoke_fiber = yield* RevokeInspection({
						connector_id: live.connector_id,
						inspection_id,
					}).pipe(
						Effect.andThen(Deferred.succeed(authority_revoked, "connector")),
						Effect.catch(() => Effect.void),
						Effect.forkDetach({ startImmediately: true }),
					);
					const revocation = yield* Deferred.await(authority_revoked);

					if (revocation === "scope") {
						yield* InterruptDetached(revoke_fiber);
					} else {
						yield* Fiber.await(scope_fiber).pipe(
							Effect.timeoutOption(teardown_timeout_ms),
							Effect.asVoid,
						);
					}

					yield* InterruptDetached(detach_fiber);
					yield* InterruptDetached(maintenance_fiber);
					yield* InterruptDetached(lease_fiber);
					yield* live.lock.withPermit(
						Effect.gen(function* () {
							const claim = yield* Ref.get(live.claim);
							const reason = yield* Ref.get(live.cleanup_reason);

							if (reason !== null) {
								yield* live.settle_cleanup(reason, claim);
							}

							yield* RemoveLive(inspection_id, live);
							yield* Deferred.succeed(live.cleanup_done, undefined);
						}),
					);
				});
			const StartLiveCleanup = (
				inspection_id: string,
				live: LiveInspection,
				reason: LiveCleanupReason | null = null,
			) =>
				Effect.gen(function* () {
					if (reason !== null) {
						yield* Ref.update(live.cleanup_reason, (current) => current ?? reason);
					}

					const already_started = yield* Ref.getAndSet(live.cleanup_started, true);

					if (already_started) {
						return;
					}

					yield* Deferred.succeed(live.shutdown, undefined);
					yield* PerformLiveCleanup(inspection_id, live).pipe(
						Effect.forkDetach({ startImmediately: true }),
					);
				}).pipe(Effect.uninterruptible);
			const CloseLive = (inspection_id: string, reason: LiveCleanupReason | null = null) =>
				Effect.gen(function* () {
					const live = (yield* Ref.get(live_inspections)).get(inspection_id);

					if (!live) {
						return Option.none<LiveCleanupResult>();
					}

					const started = yield* live.lock.withPermit(
						Effect.gen(function* () {
							const current = (yield* Ref.get(live_inspections)).get(inspection_id);

							if (current !== live) {
								return false;
							}

							yield* StartLiveCleanup(inspection_id, live, reason);

							return true;
						}),
					);

					if (!started) {
						return Option.none<LiveCleanupResult>();
					}

					const completed = yield* Deferred.await(live.cleanup_done).pipe(
						Effect.timeoutOption(teardown_timeout_ms),
					);
					const claim = yield* Ref.get(live.claim);

					return Option.some({ claim, complete: Option.isSome(completed) });
				});
			const CloseLiveFully = (inspection_id: string, reason: LiveCleanupReason) =>
				Effect.gen(function* () {
					const live = (yield* Ref.get(live_inspections)).get(inspection_id);

					if (!live) {
						return;
					}

					const started = yield* live.lock.withPermit(
						Effect.gen(function* () {
							const current = (yield* Ref.get(live_inspections)).get(inspection_id);

							if (current !== live) {
								return false;
							}

							yield* StartLiveCleanup(inspection_id, live, reason);

							return true;
						}),
					);

					if (started) {
						yield* Deferred.await(live.cleanup_done);
					}
				});
			const PublishDisconnected = (
				event: Effect.Effect<
					Option.Option<PreviewBrowserAcceptance["event"]>,
					PreviewBrowserRepositoryError
				>,
				subject_id: string,
			) =>
				event.pipe(
					MapRepositoryError(subject_id),
					Effect.flatMap(
						Option.match({
							onNone: () => Effect.void,
							onSome: PublishEvent,
						}),
					),
				);
			const RecordOwnedDisconnected = (
				inspection_id: string,
				claim: PreviewBrowserOperationClaim,
				reason: "connection_lost" | "interrupted" | "thread_erased",
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;

					yield* PublishDisconnected(
						repository.DisconnectOwnedInspection(inspection_id, claim, reason, now_ms),
						inspection_id,
					);
				});
			const RecordTargetChanged = (
				inspection_id: string,
				claim: PreviewBrowserOperationClaim,
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;

					yield* PublishDisconnected(
						repository.DisconnectChangedInspection(inspection_id, claim, now_ms),
						inspection_id,
					);
				});
			const RecordTargetRemovalDisconnect = (
				inspection_id: string,
				claim: PreviewTargetRemovalClaim,
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;

					yield* PublishDisconnected(
						repository.DisconnectTargetInspection(inspection_id, claim, now_ms),
						inspection_id,
					);
				});
			const ObserveDisconnect = (inspection_id: string, live: LiveInspection) =>
				live.session.Disconnected.pipe(
					Effect.catchCause((cause) =>
						Cause.hasInterrupts(cause) ? Effect.failCause(cause) : Effect.void,
					),
					Effect.andThen(
						live.lock.withPermit(
							Effect.gen(function* () {
								const current = (yield* Ref.get(live_inspections)).get(
									inspection_id,
								);

								if (current !== live) {
									return;
								}

								if (yield* Deferred.isDone(live.shutdown)) {
									return;
								}

								yield* StartLiveCleanup(inspection_id, live, "connection_lost");
							}),
						),
					),
					Effect.ignore,
				);
			const HeartbeatLive = (
				inspection_id: string,
				live: LiveInspection,
			): Effect.Effect<void> =>
				Effect.sleep(inspection_heartbeat_interval_ms).pipe(
					Effect.andThen(Deferred.isDone(live.shutdown)),
					Effect.flatMap((shutting_down) =>
						shutting_down
							? Effect.succeed({
									active: false,
									cleanup: false,
									reason: null,
								} as const)
							: live.lock.withPermit(
									Effect.gen(function* () {
										const current = (yield* Ref.get(live_inspections)).get(
											inspection_id,
										);

										if (current !== live) {
											return {
												active: false,
												cleanup: false,
												reason: null,
											} as const;
										}

										const claim = yield* Ref.get(live.claim);
										const now_ms = yield* clock.Now;
										const renewed = yield* repository
											.RenewInspectionLease(
												inspection_id,
												claim,
												now_ms,
												live_inspection_lease_ms,
											)
											.pipe(Effect.exit);

										if (Exit.isFailure(renewed)) {
											return {
												active: false,
												cleanup: true,
												reason: null,
											} as const;
										}

										yield* Ref.set(live.claim, renewed.value.claim);

										return renewed.value.cleanup_reason === null
											? ({
													active: true,
													cleanup: false,
													reason: null,
												} as const)
											: ({
													active: false,
													cleanup: true,
													reason: renewed.value.cleanup_reason,
												} as const);
									}),
								),
					),
					Effect.flatMap(({ active, cleanup, reason }) =>
						Effect.gen(function* () {
							if (cleanup) {
								yield* StartLiveCleanup(inspection_id, live, reason);
							}

							if (active) {
								yield* Effect.suspend(() => HeartbeatLive(inspection_id, live));
							}
						}),
					),
				);
			const RegisterLive = (inspection_id: string, live: LiveInspection) =>
				Effect.gen(function* () {
					yield* Ref.update(live_inspections, (current) =>
						new Map(current).set(inspection_id, live),
					);
					yield* Effect.forkIn(
						ObserveDisconnect(inspection_id, live),
						live.maintenance_scope,
						{ startImmediately: true, uninterruptible: false },
					);
					yield* Effect.forkIn(
						HeartbeatLive(inspection_id, live),
						live.maintenance_scope,
						{ startImmediately: true, uninterruptible: false },
					);
				});

			const AwaitLaunchPreparation = (
				command: Command,
			): Effect.Effect<ReadyLaunchPreparation, PreviewBrowserLifecycleError> =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const preparation = yield* repository
						.PrepareLaunch(command, now_ms, operation_lease_ms)
						.pipe(MapRepositoryError(command.message_id));

					if (preparation._tag !== "Pending") {
						return preparation;
					}

					yield* Effect.sleep(operation_poll_interval_ms);

					return yield* Effect.suspend(() => AwaitLaunchPreparation(command));
				});
			const SettleLaunch = (
				command: Command,
				claim: PreviewBrowserOperationClaim,
				settlement: PreviewBrowserLaunchSettlement,
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const acceptance = yield* repository
						.SettleLaunch(command, claim, settlement, now_ms)
						.pipe(MapRepositoryError(command.message_id));

					yield* PublishAcceptance(acceptance);

					return acceptance;
				});
			const OpenUnlocked = (input: Command) =>
				Effect.gen(function* () {
					const command = yield* DecodeCommand(input);

					if (!is_launch_command(command)) {
						return yield* lifecycle_error(command.message_id, "invalid_request");
					}

					const preparation = yield* AwaitLaunchPreparation(command);

					if (preparation._tag === "Completed") {
						return preparation.acceptance;
					}

					if (preparation._tag === "Interrupted") {
						return yield* SettleLaunch(command, preparation.prepared.claim, {
							reason: "interrupted",
							state: "outcome_unknown",
						});
					}

					const opened = yield* RunAdapter(
						launcher.Open(preparation.prepared.launch.url),
						launcher_timeout_ms,
					);
					const failure = Option.isSome(opened) ? typed_failure(opened.value) : undefined;
					const settlement: PreviewBrowserLaunchSettlement = Option.isNone(opened)
						? { reason: "launcher_failed", state: "outcome_unknown" }
						: Exit.isSuccess(opened.value)
							? { state: "dispatched" }
							: failure?.reason === "unavailable"
								? { reason: "launcher_unavailable", state: "rejected" }
								: failure?.reason === "rejected"
									? { reason: "launcher_rejected", state: "rejected" }
									: { reason: "launcher_failed", state: "outcome_unknown" };

					return yield* SettleLaunch(command, preparation.prepared.claim, settlement);
				});

			const AwaitInspectionPreparation = (
				command: Command,
			): Effect.Effect<ReadyInspectionPreparation, PreviewBrowserLifecycleError> =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const preparation = yield* repository
						.PrepareInspection(command, now_ms, operation_lease_ms)
						.pipe(MapRepositoryError(command.message_id));

					if (preparation._tag !== "Pending") {
						return preparation;
					}

					yield* Effect.sleep(operation_poll_interval_ms);

					return yield* Effect.suspend(() => AwaitInspectionPreparation(command));
				});
			const SettleInspection = (
				command: Command,
				claim: PreviewBrowserOperationClaim,
				settlement: PreviewInspectionAttachSettlement,
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const acceptance = yield* repository
						.SettleInspectionAttach(command, claim, settlement, now_ms)
						.pipe(MapRepositoryError(command.message_id));

					yield* PublishAcceptance(acceptance);

					return acceptance;
				});
			const AttachUnlocked = (input: Command) =>
				Effect.gen(function* () {
					const command = yield* DecodeCommand(input);

					if (!is_attach_command(command)) {
						return yield* lifecycle_error(command.message_id, "invalid_request");
					}

					const preparation = yield* AwaitInspectionPreparation(command);

					if (preparation._tag === "Completed") {
						return preparation.acceptance;
					}

					if (preparation._tag === "Interrupted") {
						yield* RevokeInspection({
							connector_id: preparation.prepared.inspection.connector_id,
							inspection_id: preparation.prepared.inspection.inspection_id,
						});

						return yield* SettleInspection(command, preparation.prepared.claim, {
							reason: "interrupted",
							state: "disconnected",
						});
					}

					const prepared = preparation.prepared;
					const revocation = {
						connector_id: prepared.inspection.connector_id,
						inspection_id: prepared.inspection.inspection_id,
					} satisfies PreviewInspectionRevocation;
					const scope = yield* Scope.make();
					let transferred = false;
					const RevokePrepared = yield* Effect.cached(RevokeInspection(revocation));
					const SettleAfterRevocation = (settlement: PreviewInspectionAttachSettlement) =>
						RevokePrepared.pipe(
							Effect.andThen(SettleInspection(command, prepared.claim, settlement)),
						);

					return yield* Effect.gen(function* () {
						const attached = yield* RunAdapter(
							connector
								.Attach({
									connector_id: prepared.inspection.connector_id,
									inspection_id: prepared.inspection.inspection_id,
									target: prepared.target,
								})
								.pipe(Scope.provide(scope)),
							connector_timeout_ms,
						);

						if (Option.isNone(attached)) {
							return yield* SettleAfterRevocation({
								reason: "connector_unavailable",
								state: "failed",
							});
						}

						if (Exit.isFailure(attached.value)) {
							const failure = typed_failure(attached.value);

							return yield* SettleAfterRevocation({
								reason:
									failure?.reason === "unavailable"
										? "connector_unavailable"
										: "connector_rejected",
								state: "failed",
							});
						}

						const session = validate_inspection_session(attached.value.value);

						if (session === undefined) {
							return yield* SettleAfterRevocation({
								reason: "connector_rejected",
								state: "failed",
							});
						}

						const acceptance = yield* SettleInspection(command, prepared.claim, {
							state: "attached",
						});

						if (
							!acceptance_is_owned_attachment(
								acceptance,
								prepared.inspection.inspection_id,
							)
						) {
							return acceptance;
						}

						const renewed_at_ms = yield* clock.Now;
						const renewal = yield* repository
							.RenewInspectionLease(
								prepared.inspection.inspection_id,
								prepared.claim,
								renewed_at_ms,
								live_inspection_lease_ms,
							)
							.pipe(MapRepositoryError(prepared.inspection.inspection_id));
						const live = {
							claim: yield* Ref.make(renewal.claim),
							cleanup_done: yield* Deferred.make<void>(),
							cleanup_reason: yield* Ref.make<LiveCleanupReason | null>(
								renewal.cleanup_reason,
							),
							cleanup_started: yield* Ref.make(false),
							connector_id: prepared.inspection.connector_id,
							lock: yield* Semaphore.make(1),
							maintenance_scope: yield* Scope.fork(service_scope),
							project_id: prepared.inspection.project_id,
							scope,
							session,
							settle_cleanup: (reason, claim) =>
								(reason === "target_changed"
									? RecordTargetChanged(prepared.inspection.inspection_id, claim)
									: RecordOwnedDisconnected(
											prepared.inspection.inspection_id,
											claim,
											reason,
										)
								).pipe(Effect.ignore),
							shutdown: yield* Deferred.make<void>(),
							target_id: prepared.inspection.target_id,
							thread_id: command.thread_id,
							workspace_id: prepared.inspection.workspace_id,
						} satisfies LiveInspection;

						yield* RegisterLive(prepared.inspection.inspection_id, live);

						if (renewal.cleanup_reason !== null) {
							yield* StartLiveCleanup(
								prepared.inspection.inspection_id,
								live,
								renewal.cleanup_reason,
							);
						}

						transferred = true;

						return acceptance;
					}).pipe(
						Effect.uninterruptible,
						Effect.ensuring(
							Effect.suspend(() =>
								transferred
									? Effect.void
									: RevokePrepared.pipe(
											Effect.ignore,
											Effect.andThen(CloseScope(scope)),
										),
							),
						),
					);
				});

			const DetachUnlocked = (input: Command) =>
				Effect.gen(function* () {
					const command = yield* DecodeCommand(input);

					if (!is_detach_command(command)) {
						return yield* lifecycle_error(command.message_id, "invalid_request");
					}

					const preparation = yield* repository
						.PrepareDetach(command)
						.pipe(MapRepositoryError(command.payload.inspection_id));

					if (preparation._tag === "Completed") {
						return preparation.acceptance;
					}

					const closure = yield* CloseLive(command.payload.inspection_id);

					if (Option.isSome(closure) && !closure.value.complete) {
						return yield* lifecycle_error(command.payload.inspection_id, "unavailable");
					}

					if (Option.isNone(closure)) {
						yield* RevokeInspection(preparation.revocation);
					}

					const now_ms = yield* clock.Now;
					const acceptance = yield* repository
						.DetachInspection(command, now_ms)
						.pipe(MapRepositoryError(command.payload.inspection_id));

					yield* PublishAcceptance(acceptance);
					return acceptance;
				});
			const DisconnectThreadIds = (inspection_ids: ReadonlyArray<string>) =>
				Effect.forEach(
					inspection_ids,
					(inspection_id) =>
						Effect.gen(function* () {
							const closure = yield* CloseLive(inspection_id, "thread_erased");

							if (Option.isSome(closure) && closure.value.complete) {
								yield* RecordOwnedDisconnected(
									inspection_id,
									closure.value.claim,
									"thread_erased",
								);

								return;
							}

							if (Option.isSome(closure)) {
								return;
							}

							if (!(yield* RevokeStoredInspection(inspection_id))) {
								return;
							}

							const now_ms = yield* clock.Now;

							yield* PublishDisconnected(
								repository.DisconnectThreadInspection(inspection_id, now_ms),
								inspection_id,
							);
						}),
					{ concurrency: 16, discard: true },
				);
			const FenceTargetUnlocked = (claim: PreviewTargetRemovalClaim) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const inspection_ids = yield* repository
						.ActiveInspectionIdsForTargetRemoval(claim, now_ms)
						.pipe(MapRepositoryError(claim.target_id));

					yield* Effect.forEach(
						inspection_ids,
						(inspection_id) =>
							Effect.gen(function* () {
								const closure = yield* CloseLive(inspection_id, "target_changed");

								if (Option.isSome(closure) && closure.value.complete) {
									yield* RecordTargetRemovalDisconnect(inspection_id, claim);

									return;
								}

								if (Option.isSome(closure)) {
									return;
								}

								if (!(yield* RevokeStoredInspection(inspection_id))) {
									return;
								}

								yield* RecordTargetRemovalDisconnect(inspection_id, claim);
							}),
						{ concurrency: 16, discard: true },
					);

					if (inspection_ids.length === 0) {
						return;
					}

					yield* Effect.sleep(operation_poll_interval_ms);

					const checked_at_ms = yield* clock.Now;
					const remaining = yield* repository
						.ActiveInspectionIdsForTargetRemoval(claim, checked_at_ms)
						.pipe(MapRepositoryError(claim.target_id));

					if (remaining.length > 0) {
						return yield* lifecycle_error(claim.target_id, "unavailable");
					}
				});
			const TargetRemovalHeartbeat = (
				claim: Ref.Ref<PreviewTargetRemovalClaim>,
				lost: Deferred.Deferred<PreviewBrowserLifecycleError>,
			): Effect.Effect<void> =>
				Effect.sleep(target_removal_heartbeat_interval_ms).pipe(
					Effect.andThen(
						Effect.gen(function* () {
							const current = yield* Ref.get(claim);
							const now_ms = yield* clock.Now;
							const renewed = yield* repository
								.RenewTargetRemoval(current, now_ms, target_removal_lease_ms)
								.pipe(MapRepositoryError(current.target_id), Effect.exit);

							if (Exit.isFailure(renewed)) {
								const failure = typed_failure(renewed);

								yield* Deferred.succeed(
									lost,
									failure ?? lifecycle_error(current.target_id, "invariant"),
								);

								return;
							}

							yield* Ref.set(claim, renewed.value);
							yield* Effect.suspend(() => TargetRemovalHeartbeat(claim, lost));
						}),
					),
				);
			const WithTargetRemovalClaim = <A, E, R>(
				claim: PreviewTargetRemovalClaim,
				use: (claim: PreviewTargetRemovalClaim) => Effect.Effect<A, E, R>,
			) =>
				Effect.gen(function* () {
					const current_claim = yield* Ref.make(claim);
					const lost = yield* Deferred.make<PreviewBrowserLifecycleError>();
					const removal_scope = yield* Scope.fork(service_scope);

					yield* Effect.forkIn(
						TargetRemovalHeartbeat(current_claim, lost),
						removal_scope,
					);

					return yield* Effect.uninterruptibleMask((restore) =>
						Effect.raceFirst(
							restore(use(claim)),
							Deferred.await(lost).pipe(
								Effect.flatMap((error) => Effect.fail(error)),
							),
						).pipe(
							Effect.ensuring(
								Scope.close(removal_scope, Exit.void).pipe(
									Effect.andThen(
										repository.ReleaseTargetRemoval(claim).pipe(Effect.ignore),
									),
								),
							),
						),
					);
				});
			const SettleTargetRemovalFenceUnlocked = (message_id: string) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const owned = yield* repository
						.ClaimTargetRemovalFence(message_id, now_ms, target_removal_lease_ms)
						.pipe(MapRepositoryError(message_id));

					if (Option.isNone(owned)) {
						return;
					}

					yield* WithTargetRemovalClaim(owned.value.claim, (claim) =>
						FenceTargetUnlocked(claim).pipe(
							Effect.andThen(clock.Now),
							Effect.flatMap((completed_at_ms) =>
								repository
									.CompleteTargetRemovalFence(
										{ ...owned.value, claim },
										completed_at_ms,
									)
									.pipe(MapRepositoryError(message_id)),
							),
						),
					);
				});
			const SynchronizeTargetRemovalUnlocked = <
				A extends PreviewTargetRemovalSettlement,
				E,
				R,
			>(
				input: {
					readonly project_id: string;
					readonly target_id: string;
					readonly workspace_id: string;
				},
				remove: (claim: PreviewTargetRemovalClaim) => Effect.Effect<A, E, R>,
			) =>
				Effect.gen(function* () {
					const now_ms = yield* clock.Now;
					const claim = yield* repository
						.ClaimTargetRemoval(input, now_ms, target_removal_lease_ms)
						.pipe(MapRepositoryError(input.target_id));

					return yield* WithTargetRemovalClaim(claim, (owned) =>
						remove(owned).pipe(
							Effect.flatMap((result) =>
								result.status === "duplicate"
									? Effect.succeed(result)
									: FenceTargetUnlocked(owned).pipe(Effect.as(result)),
							),
						),
					);
				});
			const PendingTargetRemovalFences = (thread_id?: string) =>
				repository
					.ListTargetRemovalFences(thread_id === undefined ? undefined : { thread_id })
					.pipe(MapRepositoryError(thread_id ?? "recovery"));
			const RecoverTargetRemovalFences = (thread_id?: string) =>
				PendingTargetRemovalFences(thread_id).pipe(
					Effect.flatMap((fences) =>
						Effect.forEach(
							fences,
							(fence) =>
								SettleTargetRemovalFenceUnlocked(fence.message_id).pipe(
									Effect.catchIf(
										(error) => error.code === "unavailable",
										() => Effect.void,
									),
								),
							{ concurrency: 1, discard: true },
						),
					),
				);
			const RevokeExpiredInspections = (now_ms: number) =>
				repository.ListExpiredInspectionRevocations(now_ms).pipe(
					MapRepositoryError("recovery"),
					Effect.flatMap((revocations) =>
						Effect.forEach(
							revocations,
							(revocation) =>
								RevokeInspection(revocation).pipe(
									Effect.as(Option.some(revocation.inspection_id)),
									Effect.catch(() => Effect.succeed(Option.none<string>())),
								),
							{ concurrency: 16 },
						),
					),
					Effect.map((inspection_ids) => inspection_ids.flatMap(Option.toArray)),
				);
			const RecoverOnce = Effect.gen(function* () {
				yield* RecoverTargetRemovalFences();

				const recovered_at_ms = yield* clock.Now;
				const revoked_inspection_ids = yield* RevokeExpiredInspections(recovered_at_ms);
				const recovered_events = yield* repository
					.RecoverInterrupted(recovered_at_ms, revoked_inspection_ids)
					.pipe(
						Effect.catchIf(
							(error) =>
								error instanceof PreviewBrowserRepositoryUnavailable &&
								error.reason === "ownership_lost",
							() => Effect.succeed([]),
						),
					)
					.pipe(MapRepositoryError("recovery"));

				yield* Effect.forEach(recovered_events, PublishEvent, { discard: true });
			});
			const QuiesceThreadUnlocked = (
				thread_id: string,
			): Effect.Effect<void, PreviewBrowserLifecycleError> =>
				Effect.gen(function* () {
					const fences = yield* PendingTargetRemovalFences(thread_id);

					if (fences.length > 0) {
						yield* RecoverTargetRemovalFences(thread_id);
						yield* Effect.sleep(operation_poll_interval_ms);
						return yield* QuiesceThreadUnlocked(thread_id);
					}

					const inspection_ids = yield* repository
						.ActiveInspectionIdsForThread(thread_id)
						.pipe(MapRepositoryError(thread_id));

					if (inspection_ids.length === 0) {
						return;
					}

					yield* DisconnectThreadIds(inspection_ids);
					yield* RecoverOnce;
					yield* Effect.sleep(operation_poll_interval_ms);
					yield* QuiesceThreadUnlocked(thread_id);
				});

			yield* RecoverOnce;
			yield* Effect.forkIn(
				Effect.sleep(recovery_interval_ms).pipe(
					Effect.andThen(
						Semaphore.withPermit(command_lock)(RecoverOnce).pipe(Effect.ignore),
					),
					Effect.forever,
				),
				service_scope,
			);
			yield* Effect.addFinalizer(() =>
				Semaphore.withPermit(command_lock)(
					Effect.gen(function* () {
						const inspection_ids = [...(yield* Ref.get(live_inspections)).keys()];

						yield* Effect.forEach(
							inspection_ids,
							(inspection_id) => CloseLiveFully(inspection_id, "interrupted"),
							{ concurrency: 16, discard: true },
						);
						yield* CloseScope(service_scope);
					}).pipe(Effect.ignore),
				),
			);

			return {
				Attach: (command) => Semaphore.withPermit(command_lock)(AttachUnlocked(command)),
				Detach: (command) => Semaphore.withPermit(command_lock)(DetachUnlocked(command)),
				Open: (command) => Semaphore.withPermit(command_lock)(OpenUnlocked(command)),
				Query: (input) =>
					Schema.decodeUnknownEffect(PreviewBrowserLifecycleQuery, {
						onExcessProperty: "error",
					})(input).pipe(
						Effect.mapError(() => lifecycle_error("", "invalid_request")),
						Effect.flatMap((query) =>
							repository.List(query).pipe(MapRepositoryError("")),
						),
					),
				QuiesceThread: (thread_id) =>
					Semaphore.withPermit(command_lock)(QuiesceThreadUnlocked(thread_id)),
				SettleTargetRemovalFence: (message_id) =>
					Semaphore.withPermit(command_lock)(
						SettleTargetRemovalFenceUnlocked(message_id),
					),
				SlidingEvents: Stream.fromPubSub(events),
				SynchronizeTargetRemoval: (input, remove) =>
					Semaphore.withPermit(command_lock)(
						SynchronizeTargetRemovalUnlocked(input, remove),
					),
			};
		}),
	);
}
