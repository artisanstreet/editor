import { and, asc, eq, lte, sql } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import { PreviewInspectionSessionUpdatedEvent, PreviewTargetUpdatedEvent } from "@artisan/protocol";

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
} from "../persistence/schema";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { is_local_preview_hostname } from "./network-policy";

const Identifier = Schema.String.pipe(Schema.check(Schema.isMinLength(1), Schema.isMaxLength(256)));
const TargetState = Schema.Literals(["registered", "healthy", "unhealthy", "stopped", "removed"]);
const InspectionState = Schema.Literals(["open", "closed", "abandoned"]);
const Source = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("process"), process_id: Identifier }),
	Schema.Struct({ kind: Schema.Literal("terminal"), terminal_id: Identifier }),
]);
const Routes = Schema.Array(
	Schema.String.pipe(Schema.check(Schema.isMinLength(1), Schema.isMaxLength(2_048))),
).check(Schema.isMaxLength(128));
export const PreviewTargetProjection = Schema.Struct({
	target_id: Identifier,
	thread_id: Identifier,
	project_id: Identifier,
	workspace_id: Identifier,
	url: Schema.String,
	port: Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 65_535 })),
	routes_json: Schema.String,
	source: Schema.optional(Source),
	state: TargetState,
	launch_state: Schema.Literals(["idle", "launching", "launched", "unavailable", "error"]),
	last_error: Schema.NullOr(Schema.String),
	health_json: Schema.NullOr(Schema.String),
	journal_sequence: Schema.Int.check(Schema.isGreaterThan(0)),
	created_at: Schema.String,
	updated_at: Schema.String,
	removed_at: Schema.NullOr(Schema.String),
});
export type PreviewTargetProjection = Schema.Schema.Type<typeof PreviewTargetProjection>;
export const PreviewInspectionProjection = Schema.Struct({
	session_id: Identifier,
	target_id: Identifier,
	thread_id: Identifier,
	connector_id: Identifier,
	state: InspectionState,
	reconnect_state: Schema.Literals(["connected", "reconnecting", "unavailable", "error"]),
	last_error: Schema.NullOr(Schema.String),
	journal_sequence: Schema.Int.check(Schema.isGreaterThan(0)),
	opened_at: Schema.String,
	closed_at: Schema.NullOr(Schema.String),
	updated_at: Schema.String,
});
export type PreviewInspectionProjection = Schema.Schema.Type<typeof PreviewInspectionProjection>;

export const PreviewDispatchLease = Schema.Struct({
	acquired_at: Schema.String,
	expires_at: Schema.String,
	kind: Schema.Literals(["launch", "probe", "inspection_open", "inspection_health"]),
	lease_id: Schema.String,
	owner_instance_id: Schema.String,
	session_id: Schema.NullOr(Schema.String),
	target_id: Schema.NullOr(Schema.String),
	thread_id: Schema.String,
});
export type PreviewDispatchLease = Schema.Schema.Type<typeof PreviewDispatchLease>;

/** Preview calls are bounded by adapter deadlines; this is a crash-recovery fence, not a retry window. */
export const preview_dispatch_lease_duration_ms = 60_000;

export class PreviewRepositoryError extends Data.TaggedError("PreviewRepositoryError")<{
	readonly code: "invalid" | "not_found" | "storage";
	readonly message: string;
}> {}
export class PreviewRepository extends Context.Service<
	PreviewRepository,
	{
		readonly GetTarget: (
			target_id: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly ListTargets: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly ListOpenInspections: () => Effect.Effect<
			ReadonlyArray<PreviewInspectionProjection>,
			PreviewRepositoryError
		>;
		readonly Register: (
			input: PreviewRegisterCommand,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly ReplayTargetUpdate: (
			input: PreviewTargetUpdateCommand,
		) => Effect.Effect<Option.Option<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly UpdateTarget: (
			input: PreviewTargetUpdateCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly UpdateInspection: (
			input: PreviewInspectionCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewInspectionProjection, PreviewRepositoryError>;
		readonly RecoverInspections: () => Effect.Effect<
			ReadonlyArray<PreviewInspectionProjection>,
			PreviewRepositoryError
		>;
		readonly AcquireDispatchLease: (
			input: PreviewDispatchLeaseInput,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly ReleaseDispatchLease: (lease: PreviewDispatchLease) => Effect.Effect<void>;
		readonly RenewDispatchLease: (
			lease: PreviewDispatchLease,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly RecoverDispatchLeases: () => Effect.Effect<
			ReadonlyArray<PreviewDispatchLease>,
			PreviewRepositoryError
		>;
	}
>()("Artisan/PreviewRepository") {}

export interface PreviewRegisterCommand {
	readonly message_id: string;
	readonly port: number;
	readonly project_id: string;
	readonly routes?: ReadonlyArray<string>;
	readonly source?: Schema.Schema.Type<typeof Source>;
	readonly target_id: string;
	readonly thread_id: string;
	readonly url: string;
	readonly workspace_id: string;
}
export interface PreviewTargetUpdateCommand {
	readonly action: "launch" | "probe" | "remove" | "state";
	readonly health_json?: string;
	readonly last_error?: string;
	readonly launch_state?: "idle" | "launching" | "launched" | "unavailable" | "error";
	readonly message_id: string;
	readonly state?: "healthy" | "registered" | "stopped" | "unhealthy" | "removed";
	readonly target_id: string;
	readonly thread_id: string;
}
export interface PreviewInspectionCommand {
	readonly action: "inspection_close" | "inspection_open" | "inspection_reconnect";
	readonly connector_id?: string;
	readonly last_error?: string;
	readonly message_id: string;
	readonly reconnect_state?: "connected" | "error" | "reconnecting" | "unavailable";
	readonly session_id: string;
	readonly target_id?: string;
	readonly thread_id: string;
}
export interface PreviewDispatchLeaseInput {
	readonly kind: PreviewDispatchLease["kind"];
	readonly session_id?: string;
	readonly target_id?: string;
	readonly thread_id: string;
}

const DecodeTarget = (value: unknown) =>
	Effect.try({
		try: () => {
			const row = value as typeof PreviewTargets.$inferSelect;
			Schema.decodeUnknownSync(Routes)(JSON.parse(row.routes_json));
			if (row.source_kind !== null) {
				Schema.decodeUnknownSync(Source)(
					row.source_kind === "process"
						? { kind: "process", process_id: row.source_id }
						: { kind: "terminal", terminal_id: row.source_id },
				);
			}
			return Schema.decodeUnknownSync(PreviewTargetProjection)({
				...row,
				...(row.source_kind === null
					? {}
					: {
							source:
								row.source_kind === "process"
									? { kind: "process", process_id: row.source_id }
									: { kind: "terminal", terminal_id: row.source_id },
						}),
			});
		},
		catch: () =>
			new PreviewRepositoryError({
				code: "storage",
				message: "Preview target projection is invalid",
			}),
	});
const DecodeInspection = (value: unknown) =>
	Effect.try({
		try: () => Schema.decodeUnknownSync(PreviewInspectionProjection)(value),
		catch: () =>
			new PreviewRepositoryError({
				code: "storage",
				message: "Preview inspection projection is invalid",
			}),
	});
const TargetEventPayload = (row: typeof PreviewTargets.$inferSelect) =>
	Schema.decodeUnknownSync(PreviewTargetUpdatedEvent)({
		type: "preview.target.updated",
		target: {
			id: row.target_id,
			thread_id: row.thread_id,
			workspace_id: row.workspace_id,
			project_id: row.project_id,
			url: row.url,
			port: row.port,
			routes: JSON.parse(row.routes_json),
			...(row.source_kind === null
				? {}
				: {
						source:
							row.source_kind === "process"
								? { kind: "process", process_id: row.source_id }
								: { kind: "terminal", terminal_id: row.source_id },
					}),
			state: row.state,
			launch_state: row.launch_state,
			...(row.last_error === null ? {} : { last_error: row.last_error }),
			...(row.health_json === null ? {} : { health: JSON.parse(row.health_json) }),
			journal_sequence: row.journal_sequence,
			created_at: row.created_at,
			updated_at: row.updated_at,
		},
	});
const InspectionEventPayload = (row: typeof PreviewInspectionSessions.$inferSelect) =>
	Schema.decodeUnknownSync(PreviewInspectionSessionUpdatedEvent)({
		type: "preview.inspection.updated",
		session: {
			session_id: row.session_id,
			target_id: row.target_id,
			connector_id: row.connector_id,
			state: row.state,
			reconnect_state: row.reconnect_state,
			...(row.last_error === null ? {} : { last_error: row.last_error }),
			opened_at: row.opened_at,
			...(row.closed_at === null ? {} : { closed_at: row.closed_at }),
			updated_at: row.updated_at,
		},
	});

/** Validates the durable target boundary before URLs enter the projection. */
export const ValidateLocalPreviewUrl = (value: string) =>
	Effect.try({
		try: () => new URL(value),
		catch: () =>
			new PreviewRepositoryError({ code: "invalid", message: "Preview URL is invalid" }),
	}).pipe(
		Effect.flatMap((url) =>
			(url.protocol === "http:" || url.protocol === "https:") &&
			!url.username &&
			!url.password &&
			is_local_preview_hostname(url.hostname)
				? Effect.succeed(url.href)
				: Effect.fail(
						new PreviewRepositoryError({
							code: "invalid",
							message: "Preview URL must be local HTTP(S) without credentials",
						}),
					),
		),
	);

/** Requires the declared transport port to equal the URL's canonical explicit or default port. */
export const ValidatePreviewRegistrationPort = (url: string, port: number) =>
	Effect.try({
		try: () => Schema.decodeUnknownSync(PreviewTargetProjection.fields.port)(port),
		catch: () =>
			new PreviewRepositoryError({
				code: "invalid",
				message: "Preview port is invalid",
			}),
	}).pipe(
		Effect.flatMap((declared_port) =>
			ValidateLocalPreviewUrl(url).pipe(
				Effect.flatMap((canonical_url) => {
					const parsed = new URL(canonical_url);
					const canonical_port =
						parsed.port === ""
							? parsed.protocol === "https:"
								? 443
								: 80
							: Number(parsed.port);

					return declared_port === canonical_port
						? Effect.succeed(canonical_url)
						: Effect.fail(
								new PreviewRepositoryError({
									code: "invalid",
									message: "Preview port must match the canonical URL port",
								}),
							);
				}),
			),
		),
	);

/** Reads validated durable preview projections. Mutations are owned by PreviewService. */
export const PreviewRepositoryLive = Layer.effect(
	PreviewRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
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
		const Register = (input: PreviewRegisterCommand) =>
			Effect.try({
				try: () => ({
					routes: Schema.decodeUnknownSync(Routes)(input.routes ?? []),
					source:
						input.source === undefined
							? undefined
							: Schema.decodeUnknownSync(Source)(input.source),
				}),
				catch: () =>
					new PreviewRepositoryError({
						code: "invalid",
						message: "Preview routes or source are invalid",
					}),
			}).pipe(
				Effect.flatMap(({ routes, source }) =>
					ValidatePreviewRegistrationPort(input.url, input.port).pipe(
						Effect.flatMap((url) =>
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const [command] = yield* transaction
										.select()
										.from(PreviewCommands)
										.where(eq(PreviewCommands.message_id, input.message_id))
										.limit(1);
									const payload_json = JSON.stringify(input);
									if (command) {
										if (
											command.action !== "register" ||
											command.thread_id !== input.thread_id ||
											command.payload_json !== payload_json
										)
											return yield* Effect.fail(
												new PreviewRepositoryError({
													code: "invalid",
													message:
														"Preview command ID conflicts with prior intent",
												}),
											);
										const [existing] = yield* transaction
											.select()
											.from(PreviewTargets)
											.where(eq(PreviewTargets.target_id, input.target_id))
											.limit(1);
										return existing === undefined
											? yield* Effect.fail(
													new PreviewRepositoryError({
														code: "storage",
														message:
															"Preview command has no target projection",
													}),
												)
											: yield* DecodeTarget(existing);
									}
									const [thread] = yield* transaction
										.select({ thread_id: Threads.thread_id })
										.from(Threads)
										.where(eq(Threads.thread_id, input.thread_id))
										.limit(1);
									const [erasing] = yield* transaction
										.select({ thread_id: ThreadErasureClaims.thread_id })
										.from(ThreadErasureClaims)
										.where(eq(ThreadErasureClaims.thread_id, input.thread_id))
										.limit(1);
									if (thread === undefined || erasing !== undefined)
										return yield* Effect.fail(
											new PreviewRepositoryError({
												code: "not_found",
												message:
													"Thread is unavailable for preview mutation",
											}),
										);
									const [duplicate] = yield* transaction
										.select({ target_id: PreviewTargets.target_id })
										.from(PreviewTargets)
										.where(eq(PreviewTargets.target_id, input.target_id))
										.limit(1);
									if (duplicate)
										return yield* Effect.fail(
											new PreviewRepositoryError({
												code: "invalid",
												message: "Preview target already exists",
											}),
										);
									const now = yield* metadata.Now;
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
											causation_id: input.message_id,
											correlation_id: input.message_id,
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
									yield* transaction.insert(PreviewTargets).values({
										created_at: now,
										health_json: null,
										journal_sequence: event!.sequence,
										last_error: null,
										launch_state: "idle",
										port: input.port,
										project_id: input.project_id,
										removed_at: null,
										routes_json: JSON.stringify(routes),
										source_id:
											source?.kind === "process"
												? source.process_id
												: source?.kind === "terminal"
													? source.terminal_id
													: null,
										source_kind: source?.kind ?? null,
										state: "registered",
										target_id: input.target_id,
										thread_id: input.thread_id,
										updated_at: now,
										url,
										workspace_id: input.workspace_id,
									});
									yield* transaction.insert(PreviewCommands).values({
										action: "register",
										created_at: now,
										journal_sequence: event!.sequence,
										message_id: input.message_id,
										payload_json,
										thread_id: input.thread_id,
									});
									const [stored] = yield* transaction
										.select()
										.from(PreviewTargets)
										.where(eq(PreviewTargets.target_id, input.target_id))
										.limit(1);
									yield* transaction
										.update(JournalEvents)
										.set({
											payload_json: JSON.stringify(
												TargetEventPayload(stored!),
											),
										})
										.where(eq(JournalEvents.sequence, event!.sequence));
									return yield* DecodeTarget(stored);
								}),
							),
						),
					),
				),
				Effect.mapError((error) =>
					error instanceof PreviewRepositoryError
						? error
						: new PreviewRepositoryError({
								code: "storage",
								message: "Could not register preview target",
							}),
				),
				Effect.tap((target) => notifier.Publish(target.journal_sequence)),
			);
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
						yield* transaction
							.update(PreviewTargets)
							.set({
								health_json: input.health_json ?? current.health_json,
								journal_sequence: event!.sequence,
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
							journal_sequence: event!.sequence,
							message_id: input.message_id,
							payload_json,
							thread_id: input.thread_id,
						});
						const [stored] = yield* transaction
							.select()
							.from(PreviewTargets)
							.where(eq(PreviewTargets.target_id, input.target_id))
							.limit(1);
						yield* transaction
							.update(JournalEvents)
							.set({ payload_json: JSON.stringify(TargetEventPayload(stored!)) })
							.where(eq(JournalEvents.sequence, event!.sequence));
						return yield* DecodeTarget(stored);
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
										: Effect.try({
												try: () =>
													Schema.decodeUnknownSync(
														PreviewTargetUpdatedEvent,
													)(JSON.parse(event.payload_json)).target,
												catch: () =>
													new PreviewRepositoryError({
														code: "storage",
														message:
															"Preview command journal event is invalid",
													}),
											}),
								),
								Effect.flatMap((recorded) =>
									GetTarget(input.target_id).pipe(
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
						if (input.action === "inspection_open")
							yield* transaction.insert(PreviewInspectionSessions).values({
								closed_at: null,
								connector_id: input.connector_id!,
								journal_sequence: event!.sequence,
								last_error: null,
								opened_at: now,
								reconnect_state: "connected",
								session_id: input.session_id,
								state: "open",
								target_id: input.target_id!,
								thread_id: input.thread_id,
								updated_at: now,
							});
						else
							yield* transaction
								.update(PreviewInspectionSessions)
								.set({
									closed_at: input.action === "inspection_close" ? now : null,
									journal_sequence: event!.sequence,
									last_error: input.last_error ?? null,
									reconnect_state:
										input.reconnect_state ??
										(input.action === "inspection_close"
											? "unavailable"
											: current!.reconnect_state),
									state: input.action === "inspection_close" ? "closed" : "open",
									updated_at: now,
								})
								.where(eq(PreviewInspectionSessions.session_id, input.session_id));
						yield* transaction.insert(PreviewCommands).values({
							action: input.action,
							created_at: now,
							journal_sequence: event!.sequence,
							message_id: input.message_id,
							payload_json,
							thread_id: input.thread_id,
						});
						const [stored] = yield* transaction
							.select()
							.from(PreviewInspectionSessions)
							.where(eq(PreviewInspectionSessions.session_id, input.session_id))
							.limit(1);
						yield* transaction
							.update(JournalEvents)
							.set({ payload_json: JSON.stringify(InspectionEventPayload(stored!)) })
							.where(eq(JournalEvents.sequence, event!.sequence));
						return yield* DecodeInspection(stored);
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
								yield* transaction
									.update(PreviewInspectionSessions)
									.set({
										closed_at: now,
										journal_sequence: event!.sequence,
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
								yield* transaction
									.update(JournalEvents)
									.set({
										payload_json: JSON.stringify(
											InspectionEventPayload(stored!),
										),
									})
									.where(eq(JournalEvents.sequence, event!.sequence));
								return yield* DecodeInspection(stored);
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
		const GetTarget = (target_id: string) =>
			database.client
				.select()
				.from(PreviewTargets)
				.where(eq(PreviewTargets.target_id, target_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row === undefined
							? Effect.fail(
									new PreviewRepositoryError({
										code: "not_found",
										message: "Preview target not found",
									}),
								)
							: DecodeTarget(row),
					),
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not read preview target",
								}),
					),
				);
		const ListTargets = (workspace_id?: string) =>
			(workspace_id === undefined
				? database.client
						.select()
						.from(PreviewTargets)
						.orderBy(asc(PreviewTargets.target_id))
				: database.client
						.select()
						.from(PreviewTargets)
						.where(eq(PreviewTargets.workspace_id, workspace_id))
						.orderBy(asc(PreviewTargets.target_id))
			).pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeTarget)),
				Effect.mapError((error) =>
					error instanceof PreviewRepositoryError
						? error
						: new PreviewRepositoryError({
								code: "storage",
								message: "Could not list preview targets",
							}),
				),
			);
		const ListOpenInspections = () =>
			database.client
				.select()
				.from(PreviewInspectionSessions)
				.where(eq(PreviewInspectionSessions.state, "open"))
				.orderBy(asc(PreviewInspectionSessions.opened_at))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeInspection)),
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not list preview inspections",
								}),
					),
				);
		return {
			AcquireDispatchLease,
			GetTarget,
			ListOpenInspections,
			ListTargets,
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
);
