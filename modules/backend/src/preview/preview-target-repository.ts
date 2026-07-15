import { and, asc, eq, gt } from "drizzle-orm";
import { Context, Data, DateTime, Effect, Layer, Option, Schema } from "effect";

import {
	CommandEnvelope,
	EventEnvelope,
	PreviewTargetHealth,
	PreviewTargetRecord,
	PreviewTargetUpdatedEvent,
	RawOrigin,
	type CommandEnvelope as Command,
	type PreviewTargetHealth as Health,
	type PreviewTargetRecord as Target,
	type PreviewTargetRemovedRecord as RemovedTarget,
	type PreviewTargetSource,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewTargetProbeClaims,
	PreviewTargetRemovalClaims,
	PreviewTargetRemovalFences,
	PreviewTargets,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import type { PreviewTargetRemovalClaim } from "./preview-browser";
import type {
	PreviewHealthProbeResult,
	PreviewTargetAcceptance,
	PreviewTargetRemovalReplay,
} from "./preview-target";

type TargetRow = typeof PreviewTargets.$inferSelect;
type ProbeClaimRow = typeof PreviewTargetProbeClaims.$inferSelect;
type TargetRemovalFenceRow = typeof PreviewTargetRemovalFences.$inferSelect;
type PreviewCommand = Extract<
	Command["payload"],
	{ readonly type: "preview.target.probe" | "preview.target.register" | "preview.target.remove" }
>;

interface EncodedCommand {
	readonly command_json: string;
	readonly payload_json: string;
	readonly raw_origin_json: string | null;
}

/** Identifies one exact durable probe lease acquired by this runtime. */
export interface PreviewTargetProbeClaim {
	readonly claim_token: string;
	readonly lease_expires_at: string;
	readonly message_id: string;
	readonly owner_instance_id: string;
	readonly target: Target;
	readonly target_generation_id: string;
}

/** Distinguishes completed replay, live ownership, and newly acquired work. */
export type PreviewTargetProbeClaimResult =
	| { readonly _tag: "Acquired"; readonly claim: PreviewTargetProbeClaim }
	| { readonly _tag: "Completed"; readonly acceptance: PreviewTargetAcceptance }
	| { readonly _tag: "Pending"; readonly lease_expires_at: string };

/** Reports a command identity reused with different immutable attribution or payload. */
export class PreviewTargetRepositoryConflict extends Data.TaggedError(
	"PreviewTargetRepositoryConflict",
)<{
	readonly reason: "command_conflict" | "duplicate_target" | "target_limit";
}> {}

/** Reports a target or command thread that is unavailable in the supplied scope. */
export class PreviewTargetRepositoryMissing extends Data.TaggedError(
	"PreviewTargetRepositoryMissing",
)<{
	readonly reason: "target" | "thread";
	readonly target_id: string;
}> {}

/** Reports corrupt durable preview data without exposing source internals. */
export class PreviewTargetRepositoryInvariant extends Data.TaggedError(
	"PreviewTargetRepositoryInvariant",
)<{
	readonly message: string;
}> {}

/** Reports database failures through the repository boundary. */
export class PreviewTargetRepositoryStorage extends Data.TaggedError(
	"PreviewTargetRepositoryStorage",
)<{
	readonly cause: unknown;
}> {}

/** Reports a probe lease that expired or changed before completion. */
export class PreviewTargetRepositoryUnavailable extends Data.TaggedError(
	"PreviewTargetRepositoryUnavailable",
)<{
	readonly reason: "probe_claim_lost" | "target_removing";
}> {}

export type PreviewTargetRepositoryError =
	| PreviewTargetRepositoryConflict
	| PreviewTargetRepositoryInvariant
	| PreviewTargetRepositoryMissing
	| PreviewTargetRepositoryStorage
	| PreviewTargetRepositoryUnavailable;

/** Owns current preview projections and journal-backed command replay. */
export class PreviewTargetRepository extends Context.Service<
	PreviewTargetRepository,
	{
		readonly Get: (input: {
			readonly project_id: string;
			readonly target_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<Option.Option<Target>, PreviewTargetRepositoryError>;
		readonly List: (input: {
			readonly project_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<ReadonlyArray<Target>, PreviewTargetRepositoryError>;
		readonly ClaimProbe: (
			command: Command,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewTargetProbeClaimResult, PreviewTargetRepositoryError>;
		readonly CompleteProbe: (
			command: Command,
			claim: PreviewTargetProbeClaim,
			health: PreviewHealthProbeResult,
			now_ms: number,
		) => Effect.Effect<PreviewTargetAcceptance, PreviewTargetRepositoryError>;
		readonly Register: (
			command: Command,
			url: string,
			now_ms: number,
		) => Effect.Effect<PreviewTargetAcceptance, PreviewTargetRepositoryError>;
		readonly RemoveClaimed: (
			command: Command,
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) => Effect.Effect<PreviewTargetRemovalReplay, PreviewTargetRepositoryError>;
		readonly Replay: (
			command: Command,
		) => Effect.Effect<Option.Option<PreviewTargetAcceptance>, PreviewTargetRepositoryError>;
		readonly ReplayTargetRemoval: (
			command: Command,
		) => Effect.Effect<Option.Option<PreviewTargetRemovalReplay>, PreviewTargetRepositoryError>;
		readonly ReleaseProbe: (
			claim: PreviewTargetProbeClaim,
		) => Effect.Effect<void, PreviewTargetRepositoryError>;
	}
>()("Artisan/PreviewTargetRepository") {}

function invariant(message: string): PreviewTargetRepositoryInvariant {
	return new PreviewTargetRepositoryInvariant({ message });
}

function ParseInstant(value: string, label: string) {
	return Option.match(DateTime.make(value), {
		onNone: () => Effect.fail(invariant(`${label} is invalid`)),
		onSome: Effect.succeed,
	});
}

const AddLeaseDuration = (value: string, lease_duration_ms: number) =>
	Effect.gen(function* () {
		const instant = yield* ParseInstant(value, "Preview probe lease clock");

		return DateTime.formatIso(DateTime.add(instant, { milliseconds: lease_duration_ms }));
	});

const LeaseExpired = (lease_expires_at: string, now: string) =>
	Effect.gen(function* () {
		const expiry = yield* ParseInstant(lease_expires_at, "Preview probe lease expiry");
		const current = yield* ParseInstant(now, "Preview probe lease clock");

		return DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current);
	});

function normalize_error(error: unknown): PreviewTargetRepositoryError {
	if (
		error instanceof PreviewTargetRepositoryConflict ||
		error instanceof PreviewTargetRepositoryInvariant ||
		error instanceof PreviewTargetRepositoryMissing ||
		error instanceof PreviewTargetRepositoryStorage ||
		error instanceof PreviewTargetRepositoryUnavailable
	) {
		return error;
	}

	return new PreviewTargetRepositoryStorage({ cause: error });
}

function is_preview_command(
	command: Command,
): command is Command & { readonly payload: PreviewCommand } {
	return (
		command.payload.type === "preview.target.probe" ||
		command.payload.type === "preview.target.register" ||
		command.payload.type === "preview.target.remove"
	);
}

function is_register_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.register" }>;
} {
	return command.payload.type === "preview.target.register";
}

function is_probe_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.probe" }>;
} {
	return command.payload.type === "preview.target.probe";
}

function is_remove_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.remove" }>;
} {
	return command.payload.type === "preview.target.remove";
}

function command_matches(
	command: Command,
	encoded: EncodedCommand,
	existing: typeof JournalCommands.$inferSelect,
): boolean {
	return (
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.payload_json === encoded.payload_json &&
		existing.raw_origin_json === encoded.raw_origin_json &&
		existing.run_id === (command.run_id ?? null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at &&
		existing.thread_id === command.thread_id
	);
}

const EncodeCommandPayloadJson = Schema.encodeEffect(
	Schema.fromJsonString(CommandEnvelope.fields.payload),
	{ onExcessProperty: "error" },
);
const EncodeCommandEnvelopeJson = Schema.encodeEffect(Schema.fromJsonString(CommandEnvelope), {
	onExcessProperty: "error",
});
const DecodeCommandEnvelopeJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(CommandEnvelope),
	{ onExcessProperty: "error" },
);
const EncodeRawOriginJson = Schema.encodeEffect(Schema.fromJsonString(RawOrigin), {
	onExcessProperty: "error",
});
const EncodePreviewEventJson = Schema.encodeEffect(
	Schema.fromJsonString(PreviewTargetUpdatedEvent),
	{ onExcessProperty: "error" },
);
const DecodeRawOriginJson = Schema.decodeUnknownEffect(Schema.fromJsonString(RawOrigin), {
	onExcessProperty: "error",
});
const DecodePreviewEventJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(PreviewTargetUpdatedEvent),
	{ onExcessProperty: "error" },
);

const EncodeCommand = (command: Command) =>
	Effect.gen(function* () {
		const command_json = yield* EncodeCommandEnvelopeJson(command);
		const payload_json = yield* EncodeCommandPayloadJson(command.payload);
		const raw_origin_json =
			command.raw_origin === undefined
				? null
				: yield* EncodeRawOriginJson(command.raw_origin);

		return { command_json, payload_json, raw_origin_json } satisfies EncodedCommand;
	}).pipe(Effect.mapError(() => invariant("Preview command cannot be encoded canonically")));

const DecodeSource = (row: TargetRow) =>
	Effect.gen(function* () {
		if (row.source_kind === null && row.source_id === null) {
			return undefined;
		}

		if (
			row.source_id === null ||
			(row.source_kind !== "process" && row.source_kind !== "terminal")
		) {
			return yield* invariant("Stored preview target source is corrupt");
		}

		return row.source_kind === "process"
			? ({ kind: "process", process_id: row.source_id } satisfies PreviewTargetSource)
			: ({ kind: "terminal", terminal_id: row.source_id } satisfies PreviewTargetSource);
	});

const DecodeHealth = (row: TargetRow) =>
	Effect.gen(function* () {
		const has_health = [
			row.health_status,
			row.health_checked_at_ms,
			row.health_latency_ms,
			row.health_message,
			row.health_status_code,
		].some((value) => value !== null);

		if (!has_health) {
			return undefined;
		}

		if (
			(row.health_status !== "healthy" && row.health_status !== "unhealthy") ||
			row.health_checked_at_ms === null ||
			row.health_latency_ms === null
		) {
			return yield* invariant("Stored preview target health is corrupt");
		}

		return {
			checked_at_ms: row.health_checked_at_ms,
			latency_ms: row.health_latency_ms,
			...(row.health_message === null ? {} : { message: row.health_message }),
			status: row.health_status,
			...(row.health_status_code === null ? {} : { status_code: row.health_status_code }),
		} satisfies Health;
	});

const DecodeTarget = (row: TargetRow) =>
	Effect.gen(function* () {
		const source = yield* DecodeSource(row);
		const health = yield* DecodeHealth(row);

		return yield* Schema.decodeUnknownEffect(PreviewTargetRecord, {
			onExcessProperty: "error",
		})({
			created_at_ms: row.created_at_ms,
			...(health === undefined ? {} : { health }),
			project_id: row.project_id,
			...(source === undefined ? {} : { source }),
			state: row.state,
			target_id: row.target_id,
			updated_at_ms: row.updated_at_ms,
			url: row.url,
			workspace_id: row.workspace_id,
		}).pipe(Effect.mapError(() => invariant("Stored preview target is corrupt")));
	});

/** Provides SQLite-backed preview targets using JournalCommands for exact public replay. */
export const PreviewTargetRepositoryLive = Layer.effect(
	PreviewTargetRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const DecodeEvent = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* DecodePreviewEventJson(row.payload_json);
				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* DecodeRawOriginJson(row.raw_origin_json);

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					causation_id: row.causation_id,
					correlation_id: row.correlation_id,
					journal_sequence: row.sequence,
					kind: "event",
					message_id: row.event_id,
					origin: row.origin,
					payload,
					protocol_version: 1,
					...(raw_origin === undefined ? {} : { raw_origin }),
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					schema_version: row.schema_version,
					sequence: row.stream_sequence,
					sent_at: row.occurred_at,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				});
			}).pipe(Effect.mapError(() => invariant("Stored preview event envelope is corrupt")));

		const ReadEvent = (transaction: typeof database.client, message_id: string) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.idempotency_key, message_id),
						eq(JournalEvents.event_type, "preview.target.updated"),
					),
				)
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEvent(row)
							: Effect.fail(invariant("Preview command event is missing")),
					),
				);

		const ReadTarget = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
		): Effect.Effect<{ readonly generation_id: string; readonly target: Target }, unknown> =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(PreviewTargets)
					.where(
						and(
							eq(PreviewTargets.project_id, input.project_id),
							eq(PreviewTargets.workspace_id, input.workspace_id),
							eq(PreviewTargets.target_id, input.target_id),
						),
					)
					.limit(1);

				if (!row) {
					return yield* new PreviewTargetRepositoryMissing({
						reason: "target",
						target_id: input.target_id,
					});
				}

				return {
					generation_id: row.generation_id,
					target: yield* DecodeTarget(row),
				};
			});

		const EnsureTargetNotRemoving = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
			now_ms: number,
		) =>
			Effect.gen(function* () {
				const [claim] = yield* transaction
					.select({ claim_token: PreviewTargetRemovalClaims.claim_token })
					.from(PreviewTargetRemovalClaims)
					.where(
						and(
							eq(PreviewTargetRemovalClaims.project_id, input.project_id),
							eq(PreviewTargetRemovalClaims.workspace_id, input.workspace_id),
							eq(PreviewTargetRemovalClaims.target_id, input.target_id),
							gt(PreviewTargetRemovalClaims.lease_expires_at_ms, now_ms),
						),
					)
					.limit(1);

				if (claim) {
					return yield* new PreviewTargetRepositoryUnavailable({
						reason: "target_removing",
					});
				}
			});
		const EnsureLiveTargetRemovalClaim = (
			transaction: typeof database.client,
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) =>
			Effect.gen(function* () {
				const [stored] = yield* transaction
					.select({
						claim_token: PreviewTargetRemovalClaims.claim_token,
						target_generation_id: PreviewTargetRemovalClaims.target_generation_id,
					})
					.from(PreviewTargetRemovalClaims)
					.where(
						and(
							eq(PreviewTargetRemovalClaims.project_id, claim.project_id),
							eq(PreviewTargetRemovalClaims.workspace_id, claim.workspace_id),
							eq(PreviewTargetRemovalClaims.target_id, claim.target_id),
							eq(PreviewTargetRemovalClaims.claim_token, claim.claim_token),
							eq(
								PreviewTargetRemovalClaims.owner_instance_id,
								claim.owner_instance_id,
							),
							gt(PreviewTargetRemovalClaims.lease_expires_at_ms, now_ms),
						),
					)
					.limit(1);

				const claimed_generation_id =
					claim.subject._tag === "Current" ? claim.subject.target_generation_id : null;

				if (!stored || stored.target_generation_id !== claimed_generation_id) {
					return yield* new PreviewTargetRepositoryUnavailable({
						reason: "target_removing",
					});
				}
			});

		const ReadDuplicate = (transaction: typeof database.client, message_id: string) =>
			ReadEvent(transaction, message_id).pipe(
				Effect.map((event) => ({
					event,
					status: "duplicate" as const,
				})),
			);

		const ReadTargetRemovalFence = (transaction: typeof database.client, message_id: string) =>
			transaction
				.select()
				.from(PreviewTargetRemovalFences)
				.where(eq(PreviewTargetRemovalFences.message_id, message_id))
				.limit(1)
				.pipe(
					Effect.map(([fence]) =>
						fence ? Option.some(fence) : Option.none<TargetRemovalFenceRow>(),
					),
				);

		const ValidateTargetRemovalFenceReplay = (
			fence: TargetRemovalFenceRow,
			command: Command & {
				readonly payload: Extract<
					PreviewCommand,
					{ readonly type: "preview.target.remove" }
				>;
			},
			acceptance: PreviewTargetAcceptance,
		) =>
			Effect.gen(function* () {
				const payload = acceptance.event.payload;
				const generation_id =
					payload.type === "preview.target.updated" &&
					payload.action === "removed" &&
					"generation_id" in payload.target
						? payload.target.generation_id
						: undefined;

				if (
					fence.message_id !== command.message_id ||
					fence.thread_id !== command.thread_id ||
					fence.project_id !== command.payload.project_id ||
					fence.workspace_id !== command.payload.workspace_id ||
					fence.target_id !== command.payload.target_id ||
					!Number.isSafeInteger(fence.committed_at_ms) ||
					fence.committed_at_ms < 0 ||
					payload.type !== "preview.target.updated" ||
					payload.action !== "removed" ||
					payload.target.project_id !== fence.project_id ||
					payload.target.workspace_id !== fence.workspace_id ||
					payload.target.target_id !== fence.target_id ||
					generation_id !== fence.target_generation_id
				) {
					return yield* invariant("Stored target-removal fence is corrupt");
				}
			});

		const ReplayTransaction = (
			transaction: typeof database.client,
			command: Command,
			encoded: EncodedCommand,
		) =>
			Effect.gen(function* () {
				const [existing] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, command.message_id))
					.limit(1);

				if (!existing) {
					return Option.none<PreviewTargetAcceptance>();
				}

				if (!command_matches(command, encoded, existing)) {
					return yield* new PreviewTargetRepositoryConflict({
						reason: "command_conflict",
					});
				}

				return Option.some(yield* ReadDuplicate(transaction, command.message_id));
			});

		const EnsureLiveThreadStream = (
			transaction: typeof database.client,
			command: Command & { readonly payload: PreviewCommand },
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${command.thread_id}`;
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, command.thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, command.thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, command.thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* new PreviewTargetRepositoryMissing({
						reason: "thread",
						target_id: command.payload.target_id,
					});
				}

				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);

				if (!stream) {
					return yield* new PreviewTargetRepositoryMissing({
						reason: "thread",
						target_id: command.payload.target_id,
					});
				}

				return { last_sequence: stream.last_sequence, stream_id };
			});

		const ReadProbeClaim = (transaction: typeof database.client, message_id: string) =>
			transaction
				.select()
				.from(PreviewTargetProbeClaims)
				.where(eq(PreviewTargetProbeClaims.message_id, message_id))
				.limit(1)
				.pipe(
					Effect.map(([claim]) =>
						claim ? Option.some(claim) : Option.none<ProbeClaimRow>(),
					),
				);

		const ValidateProbeClaimIntent = (
			claim: ProbeClaimRow,
			command: Command & { readonly payload: PreviewCommand },
			encoded: EncodedCommand,
		) =>
			Effect.gen(function* () {
				const stored_command = yield* DecodeCommandEnvelopeJson(claim.command_json).pipe(
					Effect.mapError(() => invariant("Stored preview probe command is corrupt")),
				);

				if (
					!is_probe_command(stored_command) ||
					stored_command.message_id !== claim.message_id ||
					stored_command.thread_id !== claim.thread_id ||
					stored_command.payload.project_id !== claim.project_id ||
					stored_command.payload.workspace_id !== claim.workspace_id ||
					stored_command.payload.target_id !== claim.target_id
				) {
					return yield* invariant("Stored preview probe claim identity is corrupt");
				}

				const canonical_command_json = yield* EncodeCommandEnvelopeJson(
					stored_command,
				).pipe(
					Effect.mapError(() =>
						invariant("Stored preview probe command cannot be encoded"),
					),
				);

				if (canonical_command_json !== claim.command_json) {
					return yield* invariant("Stored preview probe command is not canonical");
				}

				if (claim.command_json !== encoded.command_json) {
					return yield* new PreviewTargetRepositoryConflict({
						reason: "command_conflict",
					});
				}

				if (
					claim.thread_id !== command.thread_id ||
					claim.project_id !== command.payload.project_id ||
					claim.workspace_id !== command.payload.workspace_id ||
					claim.target_id !== command.payload.target_id
				) {
					return yield* invariant("Stored preview probe claim identity is corrupt");
				}
			});

		const Replay = (command: Command) =>
			Effect.gen(function* () {
				if (!is_preview_command(command)) {
					return yield* invariant("Preview repository received a non-preview command");
				}

				const encoded = yield* EncodeCommand(command);

				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* EnsureLiveThreadStream(transaction, command);
						const claim = yield* ReadProbeClaim(transaction, command.message_id);

						if (Option.isSome(claim)) {
							yield* ValidateProbeClaimIntent(claim.value, command, encoded);
						}

						return yield* ReplayTransaction(transaction, command, encoded);
					}),
				);
			}).pipe(Effect.mapError(normalize_error));

		const ReplayTargetRemoval = (command: Command) =>
			Effect.gen(function* () {
				if (!is_remove_command(command)) {
					return yield* invariant("Target-removal replay requires a remove command");
				}

				const encoded = yield* EncodeCommand(command);

				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* EnsureLiveThreadStream(transaction, command);
						const replayed = yield* ReplayTransaction(transaction, command, encoded);

						if (Option.isNone(replayed)) {
							return Option.none<PreviewTargetRemovalReplay>();
						}

						const fence = yield* ReadTargetRemovalFence(
							transaction,
							command.message_id,
						);

						if (Option.isNone(fence)) {
							return Option.some({
								...replayed.value,
								fence_status: "complete" as const,
							});
						}

						yield* ValidateTargetRemovalFenceReplay(
							fence.value,
							command,
							replayed.value,
						);

						return Option.some({
							...replayed.value,
							fence_status: "pending" as const,
						});
					}),
				);
			}).pipe(Effect.mapError(normalize_error));

		const Accept = (
			command: Command,
			action: "probed" | "registered" | "removed",
			mutate: (
				transaction: typeof database.client,
			) => Effect.Effect<Target | RemovedTarget, unknown>,
			probe_claim?: PreviewTargetProbeClaim,
		) =>
			Effect.gen(function* () {
				if (!is_preview_command(command)) {
					return yield* invariant("Preview repository received a non-preview command");
				}

				const encoded = yield* EncodeCommand(command);
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const stream = yield* EnsureLiveThreadStream(transaction, command);
							const stored_probe_claim = yield* ReadProbeClaim(
								transaction,
								command.message_id,
							);

							if (probe_claim === undefined && Option.isSome(stored_probe_claim)) {
								yield* ValidateProbeClaimIntent(
									stored_probe_claim.value,
									command,
									encoded,
								);
							}

							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (Option.isSome(replayed)) {
								return { _tag: "Duplicate" as const, acceptance: replayed.value };
							}

							const accepted_at = yield* metadata.Now;

							if (probe_claim !== undefined) {
								if (
									Option.isNone(stored_probe_claim) ||
									stored_probe_claim.value.claim_token !==
										probe_claim.claim_token ||
									stored_probe_claim.value.owner_instance_id !==
										probe_claim.owner_instance_id ||
									stored_probe_claim.value.lease_expires_at !==
										probe_claim.lease_expires_at ||
									stored_probe_claim.value.target_generation_id !==
										probe_claim.target_generation_id ||
									probe_claim.message_id !== command.message_id ||
									probe_claim.owner_instance_id !== metadata.instance_id
								) {
									return yield* new PreviewTargetRepositoryUnavailable({
										reason: "probe_claim_lost",
									});
								}

								yield* ValidateProbeClaimIntent(
									stored_probe_claim.value,
									command,
									encoded,
								);

								if (
									yield* LeaseExpired(probe_claim.lease_expires_at, accepted_at)
								) {
									return yield* new PreviewTargetRepositoryUnavailable({
										reason: "probe_claim_lost",
									});
								}
							}

							const target = yield* mutate(transaction);
							const payload = yield* Schema.decodeUnknownEffect(
								PreviewTargetUpdatedEvent,
								{ onExcessProperty: "error" },
							)({ action, target, type: "preview.target.updated" }).pipe(
								Effect.mapError(() =>
									invariant("Preview event payload is invalid"),
								),
							);
							const payload_json = yield* EncodePreviewEventJson(payload).pipe(
								Effect.mapError(() =>
									invariant("Preview event payload cannot be encoded"),
								),
							);
							const sequence = stream.last_sequence + 1;
							const event_id = yield* metadata.MakeId("event");

							yield* transaction.insert(JournalCommands).values({
								accepted_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json: encoded.payload_json,
								payload_type: command.payload.type,
								raw_origin_json: encoded.raw_origin_json,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});
							yield* transaction
								.update(EventStreams)
								.set({ last_sequence: sequence })
								.where(eq(EventStreams.stream_id, stream.stream_id));

							const [inserted] = yield* transaction
								.insert(JournalEvents)
								.values({
									agent_id: command.agent_id ?? null,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									event_id,
									event_type: payload.type,
									idempotency_key: command.message_id,
									occurred_at: accepted_at,
									origin: "backend",
									payload_json,
									raw_origin_json: encoded.raw_origin_json,
									run_id: command.run_id ?? null,
									schema_version: 1,
									stream_id: stream.stream_id,
									stream_sequence: sequence,
									thread_id: command.thread_id,
								})
								.returning({ journal_sequence: JournalEvents.sequence });

							if (!inserted) {
								return yield* invariant("Preview event was not persisted");
							}

							if (probe_claim !== undefined) {
								const [released] = yield* transaction
									.delete(PreviewTargetProbeClaims)
									.where(
										and(
											eq(
												PreviewTargetProbeClaims.message_id,
												probe_claim.message_id,
											),
											eq(
												PreviewTargetProbeClaims.claim_token,
												probe_claim.claim_token,
											),
											eq(
												PreviewTargetProbeClaims.owner_instance_id,
												probe_claim.owner_instance_id,
											),
											eq(
												PreviewTargetProbeClaims.lease_expires_at,
												probe_claim.lease_expires_at,
											),
										),
									)
									.returning({ message_id: PreviewTargetProbeClaims.message_id });

								if (!released) {
									return yield* new PreviewTargetRepositoryUnavailable({
										reason: "probe_claim_lost",
									});
								}
							}

							const event = yield* Schema.decodeUnknownEffect(EventEnvelope, {
								onExcessProperty: "error",
							})({
								...(command.agent_id === undefined
									? {}
									: { agent_id: command.agent_id }),
								causation_id: command.message_id,
								correlation_id: command.message_id,
								journal_sequence: inserted.journal_sequence,
								kind: "event",
								message_id: event_id,
								origin: "backend",
								payload,
								protocol_version: 1,
								...(command.raw_origin === undefined
									? {}
									: { raw_origin: command.raw_origin }),
								...(command.run_id === undefined ? {} : { run_id: command.run_id }),
								schema_version: 1,
								sequence,
								sent_at: accepted_at,
								stream_id: stream.stream_id,
								thread_id: command.thread_id,
							}).pipe(
								Effect.mapError(() =>
									invariant("Preview event envelope is invalid"),
								),
							);

							return { _tag: "Accepted" as const, event };
						}),
					),
				);

				if (result._tag === "Duplicate") {
					return result.acceptance;
				}

				yield* notifier.Publish(result.event.journal_sequence);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		const Get = (input: {
			readonly project_id: string;
			readonly target_id: string;
			readonly workspace_id: string;
		}) =>
			database.client
				.select()
				.from(PreviewTargets)
				.where(
					and(
						eq(PreviewTargets.project_id, input.project_id),
						eq(PreviewTargets.workspace_id, input.workspace_id),
						eq(PreviewTargets.target_id, input.target_id),
					),
				)
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeTarget(row).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none<Target>()),
					),
					Effect.mapError(normalize_error),
				);

		const List = (input: { readonly project_id: string; readonly workspace_id: string }) =>
			database.client
				.select()
				.from(PreviewTargets)
				.where(
					and(
						eq(PreviewTargets.project_id, input.project_id),
						eq(PreviewTargets.workspace_id, input.workspace_id),
					),
				)
				.orderBy(asc(PreviewTargets.target_id))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeTarget)),
					Effect.mapError(normalize_error),
				);

		const ClaimProbe = (command: Command, lease_duration_ms: number) => {
			if (!is_probe_command(command)) {
				return Effect.fail(invariant("Preview probe claim requires a probe command"));
			}

			if (
				!Number.isSafeInteger(lease_duration_ms) ||
				lease_duration_ms <= 0 ||
				lease_duration_ms > 600_000
			) {
				return Effect.fail(invariant("Preview probe lease duration is invalid"));
			}

			return Effect.gen(function* () {
				const encoded = yield* EncodeCommand(command);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThreadStream(transaction, command);

							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (Option.isSome(replayed)) {
								return {
									_tag: "Completed" as const,
									acceptance: replayed.value,
								};
							}

							const now = yield* metadata.Now;
							const now_ms = Date.parse(now);

							if (!Number.isSafeInteger(now_ms) || now_ms < 0) {
								return yield* invariant("Preview probe clock is invalid");
							}

							yield* EnsureTargetNotRemoving(transaction, command.payload, now_ms);

							const existing = yield* ReadProbeClaim(transaction, command.message_id);

							if (Option.isSome(existing)) {
								yield* ValidateProbeClaimIntent(existing.value, command, encoded);

								if (!(yield* LeaseExpired(existing.value.lease_expires_at, now))) {
									return {
										_tag: "Pending" as const,
										lease_expires_at: existing.value.lease_expires_at,
									};
								}
							}

							const target_entry = yield* ReadTarget(transaction, command.payload);
							const claim_token = yield* metadata.MakeId("claim");
							const lease_expires_at = yield* AddLeaseDuration(
								now,
								lease_duration_ms,
							);
							const claimed = Option.isSome(existing)
								? yield* transaction
										.update(PreviewTargetProbeClaims)
										.set({
											claim_token,
											command_json: encoded.command_json,
											created_at: now,
											lease_expires_at,
											owner_instance_id: metadata.instance_id,
											project_id: command.payload.project_id,
											target_id: command.payload.target_id,
											target_generation_id: target_entry.generation_id,
											thread_id: command.thread_id,
											updated_at: now,
											workspace_id: command.payload.workspace_id,
										})
										.where(
											and(
												eq(
													PreviewTargetProbeClaims.message_id,
													existing.value.message_id,
												),
												eq(
													PreviewTargetProbeClaims.command_json,
													existing.value.command_json,
												),
												eq(
													PreviewTargetProbeClaims.claim_token,
													existing.value.claim_token,
												),
												eq(
													PreviewTargetProbeClaims.owner_instance_id,
													existing.value.owner_instance_id,
												),
												eq(
													PreviewTargetProbeClaims.lease_expires_at,
													existing.value.lease_expires_at,
												),
											),
										)
										.returning({
											message_id: PreviewTargetProbeClaims.message_id,
										})
								: yield* transaction
										.insert(PreviewTargetProbeClaims)
										.values({
											claim_token,
											command_json: encoded.command_json,
											created_at: now,
											lease_expires_at,
											message_id: command.message_id,
											owner_instance_id: metadata.instance_id,
											project_id: command.payload.project_id,
											target_id: command.payload.target_id,
											target_generation_id: target_entry.generation_id,
											thread_id: command.thread_id,
											updated_at: now,
											workspace_id: command.payload.workspace_id,
										})
										.onConflictDoNothing()
										.returning({
											message_id: PreviewTargetProbeClaims.message_id,
										});

							if (!claimed[0]) {
								const current = yield* ReadProbeClaim(
									transaction,
									command.message_id,
								);

								if (Option.isNone(current)) {
									return yield* invariant(
										"Preview probe claim acquisition was lost",
									);
								}

								yield* ValidateProbeClaimIntent(current.value, command, encoded);

								return {
									_tag: "Pending" as const,
									lease_expires_at: current.value.lease_expires_at,
								};
							}

							return {
								_tag: "Acquired" as const,
								claim: {
									claim_token,
									lease_expires_at,
									message_id: command.message_id,
									owner_instance_id: metadata.instance_id,
									target: target_entry.target,
									target_generation_id: target_entry.generation_id,
								},
							};
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));
		};

		const ReleaseProbe = (claim: PreviewTargetProbeClaim) => {
			if (claim.owner_instance_id !== metadata.instance_id) {
				return Effect.void;
			}

			return RetrySqliteWrite(
				database.client
					.delete(PreviewTargetProbeClaims)
					.where(
						and(
							eq(PreviewTargetProbeClaims.message_id, claim.message_id),
							eq(PreviewTargetProbeClaims.claim_token, claim.claim_token),
							eq(PreviewTargetProbeClaims.owner_instance_id, claim.owner_instance_id),
							eq(PreviewTargetProbeClaims.lease_expires_at, claim.lease_expires_at),
						),
					)
					.pipe(Effect.asVoid),
			).pipe(Effect.mapError(normalize_error));
		};

		const Register = (command: Command, url: string, now_ms: number) => {
			if (!is_register_command(command)) {
				return Effect.fail(invariant("Preview registration requires a register command"));
			}

			const { payload } = command;
			const target: Target = {
				created_at_ms: now_ms,
				...(payload.source === undefined ? {} : { source: payload.source }),
				project_id: payload.project_id,
				state: "registered",
				target_id: payload.target_id,
				updated_at_ms: now_ms,
				url,
				workspace_id: payload.workspace_id,
			};

			return Accept(command, "registered", (transaction) =>
				Effect.gen(function* () {
					yield* EnsureTargetNotRemoving(transaction, payload, now_ms);

					const [existing] = yield* transaction
						.select({ target_id: PreviewTargets.target_id })
						.from(PreviewTargets)
						.where(
							and(
								eq(PreviewTargets.project_id, payload.project_id),
								eq(PreviewTargets.workspace_id, payload.workspace_id),
								eq(PreviewTargets.target_id, payload.target_id),
							),
						)
						.limit(1);

					if (existing) {
						return yield* new PreviewTargetRepositoryConflict({
							reason: "duplicate_target",
						});
					}

					const scoped_targets = yield* transaction
						.select({ target_id: PreviewTargets.target_id })
						.from(PreviewTargets)
						.where(
							and(
								eq(PreviewTargets.project_id, payload.project_id),
								eq(PreviewTargets.workspace_id, payload.workspace_id),
							),
						)
						.limit(256);

					if (scoped_targets.length >= 256) {
						return yield* new PreviewTargetRepositoryConflict({
							reason: "target_limit",
						});
					}

					const generation_id = yield* metadata.MakeId("preview_target");

					yield* transaction.insert(PreviewTargets).values({
						created_at_ms: target.created_at_ms,
						generation_id,
						project_id: target.project_id,
						source_id:
							target.source?.kind === "process"
								? target.source.process_id
								: target.source?.kind === "terminal"
									? target.source.terminal_id
									: null,
						source_kind: target.source?.kind ?? null,
						state: target.state,
						target_id: target.target_id,
						updated_at_ms: target.updated_at_ms,
						url: target.url,
						workspace_id: target.workspace_id,
					});

					return target;
				}),
			);
		};

		const CompleteProbe = (
			command: Command,
			claim: PreviewTargetProbeClaim,
			observation: PreviewHealthProbeResult,
			now_ms: number,
		) => {
			if (!is_probe_command(command)) {
				return Effect.fail(invariant("Preview probe requires a probe command"));
			}

			const { payload } = command;

			return Accept(
				command,
				"probed",
				(transaction) =>
					Effect.gen(function* () {
						yield* EnsureTargetNotRemoving(transaction, payload, now_ms);

						const health: Health = yield* Schema.decodeUnknownEffect(
							PreviewTargetHealth,
							{
								onExcessProperty: "error",
							},
						)({
							checked_at_ms: now_ms,
							latency_ms: observation.latency_ms,
							...(Option.isNone(observation.message)
								? {}
								: { message: observation.message.value }),
							status: observation.status,
							...(Option.isNone(observation.status_code)
								? {}
								: { status_code: observation.status_code.value }),
						}).pipe(
							Effect.mapError(() =>
								invariant("Preview health observation is invalid"),
							),
						);
						const target_entry = yield* ReadTarget(transaction, payload);

						if (target_entry.generation_id !== claim.target_generation_id) {
							return yield* new PreviewTargetRepositoryUnavailable({
								reason: "probe_claim_lost",
							});
						}

						const { target } = target_entry;
						const next: Target = {
							...target,
							health,
							state: health.status,
							updated_at_ms: now_ms,
						};

						yield* transaction
							.update(PreviewTargets)
							.set({
								health_checked_at_ms: health.checked_at_ms,
								health_latency_ms: health.latency_ms,
								health_message: health.message ?? null,
								health_status: health.status,
								health_status_code: health.status_code ?? null,
								state: health.status,
								updated_at_ms: now_ms,
							})
							.where(
								and(
									eq(PreviewTargets.project_id, payload.project_id),
									eq(PreviewTargets.workspace_id, payload.workspace_id),
									eq(PreviewTargets.target_id, payload.target_id),
								),
							);

						return next;
					}),
				claim,
			);
		};

		const RemoveClaimed = (
			command: Command,
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) => {
			if (!is_remove_command(command)) {
				return Effect.fail(invariant("Claimed preview removal requires a remove command"));
			}

			const { payload } = command;

			if (
				claim.project_id !== payload.project_id ||
				claim.target_id !== payload.target_id ||
				claim.workspace_id !== payload.workspace_id
			) {
				return Effect.fail(invariant("Claimed preview removal has the wrong target"));
			}

			return Effect.gen(function* () {
				const acceptance = yield* Accept(command, "removed", (transaction) =>
					Effect.gen(function* () {
						yield* EnsureLiveTargetRemovalClaim(transaction, claim, now_ms);

						if (claim.subject._tag === "Missing") {
							return yield* new PreviewTargetRepositoryMissing({
								reason: "target",
								target_id: payload.target_id,
							});
						}

						const { target } = yield* ReadTarget(transaction, payload);
						const removed = {
							...target,
							generation_id: claim.subject.target_generation_id,
							state: "removed" as const,
							updated_at_ms: now_ms,
						};
						const [deleted] = yield* transaction
							.delete(PreviewTargets)
							.where(
								and(
									eq(PreviewTargets.project_id, payload.project_id),
									eq(PreviewTargets.workspace_id, payload.workspace_id),
									eq(PreviewTargets.target_id, payload.target_id),
									eq(
										PreviewTargets.generation_id,
										claim.subject.target_generation_id,
									),
								),
							)
							.returning({ target_id: PreviewTargets.target_id });

						if (!deleted) {
							return yield* new PreviewTargetRepositoryUnavailable({
								reason: "target_removing",
							});
						}

						const [fence] = yield* transaction
							.insert(PreviewTargetRemovalFences)
							.values({
								committed_at_ms: now_ms,
								message_id: command.message_id,
								project_id: payload.project_id,
								target_generation_id: claim.subject.target_generation_id,
								target_id: payload.target_id,
								thread_id: command.thread_id,
								workspace_id: payload.workspace_id,
							})
							.onConflictDoNothing()
							.returning({ message_id: PreviewTargetRemovalFences.message_id });

						if (!fence) {
							return yield* new PreviewTargetRepositoryUnavailable({
								reason: "target_removing",
							});
						}

						return removed;
					}),
				);
				const replayed = yield* ReplayTargetRemoval(command);

				if (Option.isNone(replayed)) {
					return yield* invariant("Committed target removal cannot be replayed");
				}

				if (replayed.value.event.message_id !== acceptance.event.message_id) {
					return yield* invariant("Target-removal replay event changed after commit");
				}

				return { ...replayed.value, status: acceptance.status };
			});
		};

		return {
			ClaimProbe,
			CompleteProbe,
			Get,
			List,
			Register,
			ReleaseProbe,
			RemoveClaimed,
			Replay,
			ReplayTargetRemoval,
		};
	}),
);
