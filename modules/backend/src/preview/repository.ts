import { and, eq, lte, sql } from "drizzle-orm";
import { Effect, Layer, Option, Schema } from "effect";
import { Database } from "../persistence/database";
import {
	EventStreams,
	JournalEvents,
	PreviewCommands,
	PreviewDispatchLeases,
	PreviewInspectionSessions,
	PreviewTargets,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
} from "../persistence/tables";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	PreviewDispatchLease,
	type PreviewDispatchLeaseInput,
	type PreviewInspectionCommand,
	PreviewRepository,
	PreviewRepositoryError,
	type PreviewTargetUpdateCommand,
	preview_dispatch_lease_duration_ms,
} from "./contracts";
import { PreviewReader, PreviewReaderLive } from "./reader";
import { PreviewRegistration, PreviewRegistrationLive } from "./registration";
import {
	DecodeInspection,
	DecodeTarget,
	DecodeTargetEvent,
	EncodeInspectionEvent,
	EncodeTargetEvent,
	RequireStored,
} from "./storage-codec";

export * from "./contracts";
export * from "./validation";

/** Reads validated durable preview projections. Mutations are owned by PreviewService. */
export const PreviewRepositoryLive = Layer.effect(
	PreviewRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const reader = yield* PreviewReader;
		const registration = yield* PreviewRegistration;
		const AcquireDispatchLease = (input: PreviewDispatchLeaseInput) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						yield* transaction
							.delete(PreviewDispatchLeases)
							.where(lte(PreviewDispatchLeases.expires_at, now));
						const [claim, tombstone, thread, active] = yield* Effect.all([
							transaction
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
								.limit(1),
							transaction
								.select({ thread_id: ThreadTombstones.thread_id })
								.from(ThreadTombstones)
								.where(eq(ThreadTombstones.thread_id, input.thread_id))
								.limit(1),
							transaction
								.select({ thread_id: Threads.thread_id })
								.from(Threads)
								.where(eq(Threads.thread_id, input.thread_id))
								.limit(1),
							transaction
								.select({ lease_id: PreviewDispatchLeases.lease_id })
								.from(PreviewDispatchLeases)
								.where(eq(PreviewDispatchLeases.thread_id, input.thread_id))
								.limit(1),
						]);
						if (claim[0] || tombstone[0] || thread[0] === undefined || active[0])
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Thread is unavailable for preview dispatch",
								}),
							);
						const lease: PreviewDispatchLease = {
							acquired_at: now,
							expires_at: new Date(
								Date.parse(now) + preview_dispatch_lease_duration_ms,
							).toISOString(),
							kind: input.kind,
							lease_id: `preview_lease_${yield* metadata.MakeId("event")}`,
							owner_instance_id: metadata.instance_id,
							session_id: input.session_id ?? null,
							target_id: input.target_id ?? null,
							thread_id: input.thread_id,
						};
						yield* transaction.insert(PreviewDispatchLeases).values(lease);
						return lease;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not acquire preview dispatch lease",
								}),
					),
				);
		const ReleaseDispatchLease = (lease: PreviewDispatchLease) =>
			database.client
				.delete(PreviewDispatchLeases)
				.where(
					and(
						eq(PreviewDispatchLeases.lease_id, lease.lease_id),
						eq(PreviewDispatchLeases.owner_instance_id, metadata.instance_id),
					),
				)
				.pipe(
					Effect.asVoid,
					Effect.catch(() => Effect.void),
				);
		const RenewDispatchLease = (lease: PreviewDispatchLease) =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				const expires_at = new Date(
					Date.parse(now) + preview_dispatch_lease_duration_ms,
				).toISOString();
				const [renewed] = yield* database.client
					.update(PreviewDispatchLeases)
					.set({ expires_at })
					.where(
						and(
							eq(PreviewDispatchLeases.lease_id, lease.lease_id),
							eq(PreviewDispatchLeases.owner_instance_id, metadata.instance_id),
							eq(PreviewDispatchLeases.thread_id, lease.thread_id),
							sql`${PreviewDispatchLeases.expires_at} > ${now}`,
						),
					)
					.returning();
				if (renewed === undefined)
					return yield* Effect.fail(
						new PreviewRepositoryError({
							code: "not_found",
							message: "Preview dispatch lease is no longer owned",
						}),
					);
				return yield* Effect.try({
					try: () => Schema.decodeUnknownSync(PreviewDispatchLease)(renewed),
					catch: () =>
						new PreviewRepositoryError({
							code: "storage",
							message: "Renewed preview dispatch lease is invalid",
						}),
				});
			}).pipe(
				Effect.mapError((error) =>
					error instanceof PreviewRepositoryError
						? error
						: new PreviewRepositoryError({
								code: "storage",
								message: "Could not renew preview dispatch lease",
							}),
				),
			);
		const Register = registration.Register;
		const UpdateTarget = (input: PreviewTargetUpdateCommand, dispatch_lease_id?: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const payload_json = JSON.stringify(input);
						const [command] = yield* transaction
							.select()
							.from(PreviewCommands)
							.where(eq(PreviewCommands.message_id, input.message_id))
							.limit(1);
						if (command) {
							if (
								command.action !== input.action ||
								command.thread_id !== input.thread_id ||
								command.payload_json !== payload_json
							)
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "invalid",
										message: "Preview command ID conflicts with prior intent",
									}),
								);
							const [stored] = yield* transaction
								.select()
								.from(PreviewTargets)
								.where(eq(PreviewTargets.target_id, input.target_id))
								.limit(1);
							return stored === undefined
								? yield* Effect.fail(
										new PreviewRepositoryError({
											code: "storage",
											message: "Preview command has no target projection",
										}),
									)
								: yield* DecodeTarget(stored);
						}
						const now = yield* metadata.Now;
						const [claim, dispatch_lease] = yield* Effect.all([
							transaction
								.select()
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
								.limit(1),
							dispatch_lease_id === undefined
								? Effect.succeed(undefined)
								: transaction
										.select()
										.from(PreviewDispatchLeases)
										.where(
											and(
												eq(
													PreviewDispatchLeases.lease_id,
													dispatch_lease_id,
												),
												eq(
													PreviewDispatchLeases.thread_id,
													input.thread_id,
												),
												eq(
													PreviewDispatchLeases.target_id,
													input.target_id,
												),
												eq(
													PreviewDispatchLeases.owner_instance_id,
													metadata.instance_id,
												),
												sql`${PreviewDispatchLeases.expires_at} > ${now}`,
											),
										)
										.limit(1)
										.pipe(Effect.map(([row]) => row)),
						]);
						const [current] = yield* transaction
							.select()
							.from(PreviewTargets)
							.where(
								and(
									eq(PreviewTargets.target_id, input.target_id),
									eq(PreviewTargets.thread_id, input.thread_id),
								),
							)
							.limit(1);
						if ((claim[0] && dispatch_lease === undefined) || current === undefined)
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Preview target is unavailable",
								}),
							);
						if (current.state === "removed")
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Preview target was removed",
								}),
							);
						if (
							input.action === "launch" &&
							input.launch_state === "launching" &&
							current.launch_state === "launching"
						)
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "invalid",
									message: "Preview target launch is already claimed",
								}),
							);
						const stream_id = `thread:${input.thread_id}`;
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);
						const stream_sequence = (stream?.last_sequence ?? 0) + 1;
						if (stream)
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: stream_sequence })
								.where(eq(EventStreams.stream_id, stream_id));
						else
							yield* transaction
								.insert(EventStreams)
								.values({ stream_id, last_sequence: stream_sequence });
						const event_id = yield* metadata.MakeId("event");
						const [event] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: null,
								causation_id: dispatch_lease_id ?? input.message_id,
								correlation_id: dispatch_lease_id ?? input.message_id,
								event_id,
								event_type: "preview.target.updated",
								occurred_at: now,
								origin: "backend",
								payload_json: "{}",
								raw_origin_json: null,
								run_id: null,
								schema_version: 1,
								stream_id,
								stream_sequence,
								thread_id: input.thread_id,
							})
							.returning({ sequence: JournalEvents.sequence });
						const persisted_event = yield* RequireStored(
							event,
							"Target event insert returned no row",
						);
						yield* transaction
							.update(PreviewTargets)
							.set({
								health_json: input.health_json ?? current.health_json,
								journal_sequence: persisted_event.sequence,
								last_error: input.last_error ?? null,
								launch_state: input.launch_state ?? current.launch_state,
								removed_at: input.action === "remove" ? now : current.removed_at,
								state:
									input.state ??
									(input.action === "remove" ? "removed" : current.state),
								updated_at: now,
							})
							.where(eq(PreviewTargets.target_id, input.target_id));
						yield* transaction.insert(PreviewCommands).values({
							action: input.action,
							created_at: now,
							journal_sequence: persisted_event.sequence,
							message_id: input.message_id,
							payload_json,
							thread_id: input.thread_id,
						});
						const [stored] = yield* transaction
							.select()
							.from(PreviewTargets)
							.where(eq(PreviewTargets.target_id, input.target_id))
							.limit(1);
						const persisted_target = yield* RequireStored(
							stored,
							"Target update returned no row",
						);
						yield* transaction
							.update(JournalEvents)
							.set({ payload_json: EncodeTargetEvent(persisted_target) })
							.where(eq(JournalEvents.sequence, persisted_event.sequence));
						return yield* DecodeTarget(persisted_target);
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not update preview target",
								}),
					),
					Effect.tap((target) => notifier.Publish(target.journal_sequence)),
				);
		const ReplayTargetUpdate = (input: PreviewTargetUpdateCommand) =>
			database.client
				.select()
				.from(PreviewCommands)
				.where(eq(PreviewCommands.message_id, input.message_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([command]) => {
						if (command === undefined) return Effect.succeed(Option.none());
						if (
							command.action !== input.action ||
							command.thread_id !== input.thread_id ||
							command.payload_json !== JSON.stringify(input)
						)
							return Effect.fail(
								new PreviewRepositoryError({
									code: "invalid",
									message: "Preview command ID conflicts with prior intent",
								}),
							);
						return database.client
							.select({ payload_json: JournalEvents.payload_json })
							.from(JournalEvents)
							.where(eq(JournalEvents.sequence, command.journal_sequence))
							.limit(1)
							.pipe(
								Effect.flatMap(([event]) =>
									event === undefined
										? Effect.fail(
												new PreviewRepositoryError({
													code: "storage",
													message: "Preview command has no journal event",
												}),
											)
										: DecodeTargetEvent(event.payload_json).pipe(
												Effect.map((payload) => payload.target),
											),
								),
								Effect.flatMap((recorded) =>
									reader.GetTarget(input.target_id).pipe(
										Effect.map((current) =>
											Option.some({
												...current,
												journal_sequence: command.journal_sequence,
												launch_state: recorded.launch_state,
												updated_at: recorded.updated_at,
											}),
										),
									),
								),
							);
					}),
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not replay preview target command",
								}),
					),
				);
		const UpdateInspection = (input: PreviewInspectionCommand, dispatch_lease_id?: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const payload_json = JSON.stringify(input);
						const [command] = yield* transaction
							.select()
							.from(PreviewCommands)
							.where(eq(PreviewCommands.message_id, input.message_id))
							.limit(1);
						if (command) {
							if (
								command.action !== input.action ||
								command.thread_id !== input.thread_id ||
								command.payload_json !== payload_json
							)
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "invalid",
										message: "Preview command ID conflicts with prior intent",
									}),
								);
							const [stored] = yield* transaction
								.select()
								.from(PreviewInspectionSessions)
								.where(eq(PreviewInspectionSessions.session_id, input.session_id))
								.limit(1);
							return stored === undefined
								? yield* Effect.fail(
										new PreviewRepositoryError({
											code: "storage",
											message: "Preview command has no inspection projection",
										}),
									)
								: yield* DecodeInspection(stored);
						}
						const now = yield* metadata.Now;
						const [claim, dispatch_lease] = yield* Effect.all([
							transaction
								.select()
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
								.limit(1),
							dispatch_lease_id === undefined
								? Effect.succeed(undefined)
								: transaction
										.select()
										.from(PreviewDispatchLeases)
										.where(
											and(
												eq(
													PreviewDispatchLeases.lease_id,
													dispatch_lease_id,
												),
												eq(
													PreviewDispatchLeases.thread_id,
													input.thread_id,
												),
												eq(
													PreviewDispatchLeases.session_id,
													input.session_id,
												),
												eq(
													PreviewDispatchLeases.owner_instance_id,
													metadata.instance_id,
												),
												sql`${PreviewDispatchLeases.expires_at} > ${now}`,
											),
										)
										.limit(1)
										.pipe(Effect.map(([row]) => row)),
						]);
						if (claim[0] && dispatch_lease === undefined)
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Thread is unavailable for preview inspection",
								}),
							);
						const [current] = yield* transaction
							.select()
							.from(PreviewInspectionSessions)
							.where(eq(PreviewInspectionSessions.session_id, input.session_id))
							.limit(1);
						if (input.action === "inspection_open" && current !== undefined)
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "invalid",
									message: "Inspection session already exists",
								}),
							);
						if (
							input.action !== "inspection_open" &&
							(current === undefined ||
								current.thread_id !== input.thread_id ||
								current.state !== "open")
						)
							return yield* Effect.fail(
								new PreviewRepositoryError({
									code: "not_found",
									message: "Open inspection session not found",
								}),
							);
						if (input.action === "inspection_open") {
							if (!input.target_id || !input.connector_id)
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "invalid",
										message:
											"Inspection open requires target and connector IDs",
									}),
								);
							const [target] = yield* transaction
								.select()
								.from(PreviewTargets)
								.where(
									and(
										eq(PreviewTargets.target_id, input.target_id),
										eq(PreviewTargets.thread_id, input.thread_id),
									),
								)
								.limit(1);
							if (target === undefined || target.state === "removed")
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "not_found",
										message: "Preview target not found",
									}),
								);
						}
						const stream_id = `thread:${input.thread_id}`;
						const [stream] = yield* transaction
							.select()
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);
						const stream_sequence = (stream?.last_sequence ?? 0) + 1;
						if (stream)
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: stream_sequence })
								.where(eq(EventStreams.stream_id, stream_id));
						else
							yield* transaction
								.insert(EventStreams)
								.values({ stream_id, last_sequence: stream_sequence });
						const event_id = yield* metadata.MakeId("event");
						const [event] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: null,
								causation_id: dispatch_lease_id ?? input.message_id,
								correlation_id: dispatch_lease_id ?? input.message_id,
								event_id,
								event_type: "preview.inspection.updated",
								occurred_at: now,
								origin: "backend",
								payload_json: "{}",
								raw_origin_json: null,
								run_id: null,
								schema_version: 1,
								stream_id,
								stream_sequence,
								thread_id: input.thread_id,
							})
							.returning({ sequence: JournalEvents.sequence });
						const persisted_event = yield* RequireStored(
							event,
							"Inspection event insert returned no row",
						);
						if (input.action === "inspection_open") {
							const connector_id = input.connector_id;
							const target_id = input.target_id;
							if (connector_id === undefined || target_id === undefined)
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "invalid",
										message:
											"Inspection open requires target and connector IDs",
									}),
								);
							yield* transaction.insert(PreviewInspectionSessions).values({
								closed_at: null,
								connector_id,
								journal_sequence: persisted_event.sequence,
								last_error: null,
								opened_at: now,
								reconnect_state: "connected",
								session_id: input.session_id,
								state: "open",
								target_id,
								thread_id: input.thread_id,
								updated_at: now,
							});
						} else {
							const persisted_inspection = yield* RequireStored(
								current,
								"Open inspection row is missing",
							);
							yield* transaction
								.update(PreviewInspectionSessions)
								.set({
									closed_at: input.action === "inspection_close" ? now : null,
									journal_sequence: persisted_event.sequence,
									last_error: input.last_error ?? null,
									reconnect_state:
										input.reconnect_state ??
										(input.action === "inspection_close"
											? "unavailable"
											: persisted_inspection.reconnect_state),
									state: input.action === "inspection_close" ? "closed" : "open",
									updated_at: now,
								})
								.where(eq(PreviewInspectionSessions.session_id, input.session_id));
						}
						yield* transaction.insert(PreviewCommands).values({
							action: input.action,
							created_at: now,
							journal_sequence: persisted_event.sequence,
							message_id: input.message_id,
							payload_json,
							thread_id: input.thread_id,
						});
						const [stored] = yield* transaction
							.select()
							.from(PreviewInspectionSessions)
							.where(eq(PreviewInspectionSessions.session_id, input.session_id))
							.limit(1);
						const persisted_inspection = yield* RequireStored(
							stored,
							"Inspection update returned no row",
						);
						yield* transaction
							.update(JournalEvents)
							.set({ payload_json: EncodeInspectionEvent(persisted_inspection) })
							.where(eq(JournalEvents.sequence, persisted_event.sequence));
						return yield* DecodeInspection(persisted_inspection);
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not update preview inspection",
								}),
					),
					Effect.tap((inspection) => notifier.Publish(inspection.journal_sequence)),
				);
		/** Removes only expired leases after a crash. Callers terminalize the recorded intent;
		 * they must never replay the external action that may already have escaped. */
		const RecoverDispatchLeases = () =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				const expired = yield* database.client
					.delete(PreviewDispatchLeases)
					.where(lte(PreviewDispatchLeases.expires_at, now))
					.returning();
				return yield* Effect.forEach(expired, (lease) =>
					Effect.try({
						try: () => Schema.decodeUnknownSync(PreviewDispatchLease)(lease),
						catch: () =>
							new PreviewRepositoryError({
								code: "storage",
								message: "Preview dispatch lease is invalid",
							}),
					}),
				);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof PreviewRepositoryError
						? error
						: new PreviewRepositoryError({
								code: "storage",
								message: "Could not recover preview dispatch leases",
							}),
				),
			);
		const RecoverInspections = () =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const rows = yield* transaction
							.select()
							.from(PreviewInspectionSessions)
							.where(eq(PreviewInspectionSessions.state, "open"));
						return yield* Effect.forEach(rows, (row) =>
							Effect.gen(function* () {
								const stream_id = `thread:${row.thread_id}`;
								const [stream] = yield* transaction
									.select()
									.from(EventStreams)
									.where(eq(EventStreams.stream_id, stream_id))
									.limit(1);
								const stream_sequence = (stream?.last_sequence ?? 0) + 1;
								if (stream)
									yield* transaction
										.update(EventStreams)
										.set({ last_sequence: stream_sequence })
										.where(eq(EventStreams.stream_id, stream_id));
								else
									yield* transaction
										.insert(EventStreams)
										.values({ stream_id, last_sequence: stream_sequence });
								const event_id = yield* metadata.MakeId("event");
								const [event] = yield* transaction
									.insert(JournalEvents)
									.values({
										agent_id: null,
										causation_id: row.session_id,
										correlation_id: row.session_id,
										event_id,
										event_type: "preview.inspection.updated",
										occurred_at: now,
										origin: "backend",
										payload_json: "{}",
										raw_origin_json: null,
										run_id: null,
										schema_version: 1,
										stream_id,
										stream_sequence,
										thread_id: row.thread_id,
									})
									.returning({ sequence: JournalEvents.sequence });
								const persisted_event = yield* RequireStored(
									event,
									"Recovery event insert returned no row",
								);
								yield* transaction
									.update(PreviewInspectionSessions)
									.set({
										closed_at: now,
										journal_sequence: persisted_event.sequence,
										last_error: "backend_restart",
										reconnect_state: "unavailable",
										state: "abandoned",
										updated_at: now,
									})
									.where(
										eq(PreviewInspectionSessions.session_id, row.session_id),
									);
								const [stored] = yield* transaction
									.select()
									.from(PreviewInspectionSessions)
									.where(eq(PreviewInspectionSessions.session_id, row.session_id))
									.limit(1);
								const persisted_inspection = yield* RequireStored(
									stored,
									"Recovery update returned no row",
								);
								yield* transaction
									.update(JournalEvents)
									.set({
										payload_json: EncodeInspectionEvent(persisted_inspection),
									})
									.where(eq(JournalEvents.sequence, persisted_event.sequence));
								return yield* DecodeInspection(persisted_inspection);
							}),
						);
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not recover preview inspections",
								}),
					),
				);
		return {
			AcquireDispatchLease,
			GetTarget: reader.GetTarget,
			ListOpenInspections: reader.ListOpenInspections,
			ListTargets: reader.ListTargets,
			RecoverDispatchLeases,
			RecoverInspections,
			ReleaseDispatchLease,
			RenewDispatchLease,
			Register,
			ReplayTargetUpdate,
			UpdateInspection,
			UpdateTarget,
		};
	}),
).pipe(Layer.provide(Layer.merge(PreviewReaderLive, PreviewRegistrationLive)));
