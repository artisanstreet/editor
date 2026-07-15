import { and, asc, desc, eq, gt, inArray, lte, ne } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	CommandEnvelope,
	EventEnvelope,
	PreviewBrowserLaunchRecord,
	PreviewBrowserLifecycleEvent,
	PreviewBrowserLifecycleQueryResult,
	PreviewInspectionSessionRecord,
	PreviewTargetRecord,
	PreviewTargetUpdatedEvent,
	RawOrigin,
	type CommandEnvelope as Command,
	type PreviewBrowserInitiator,
	type PreviewBrowserLifecycleEvent as LifecycleEvent,
	type PreviewBrowserLifecycleQueryResult as LifecycleQueryResult,
	type PreviewTargetHealth,
	type PreviewTargetSource,
} from "@artisan/protocol";

import {
	PreviewBrowserLifecycleError,
	preview_browser_initiator,
	type PreviewBrowserAcceptance,
	type PreviewBrowserOperationClaim,
	type PendingPreviewTargetRemovalFence,
	type OwnedPreviewTargetRemovalFence,
	type PreviewTargetRemovalClaim,
	type PreparedPreviewBrowserLaunch,
	type PreparedPreviewInspection,
	type PreviewInspectionRevocation,
} from "./preview-browser";
import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewBrowserLaunches,
	PreviewInspectionSessions,
	PreviewTargetProbeClaims,
	PreviewTargetRemovalClaims,
	PreviewTargetRemovalFences,
	PreviewTargets,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

type LaunchRow = typeof PreviewBrowserLaunches.$inferSelect;
type InspectionRow = typeof PreviewInspectionSessions.$inferSelect;
type TargetRow = typeof PreviewTargets.$inferSelect;
type TargetRemovalFenceRow = typeof PreviewTargetRemovalFences.$inferSelect;
type BrowserPayload = Extract<
	Command["payload"],
	{
		readonly type:
			| "preview.browser.open"
			| "preview.inspection.attach"
			| "preview.inspection.detach";
	}
>;
type BrowserCommand = Command & { readonly payload: BrowserPayload };

interface EncodedCommand {
	readonly command_json: string;
	readonly payload_json: string;
	readonly raw_origin_json: string | null;
}

type BrowserCommandReplay =
	| { readonly _tag: "Completed"; readonly acceptance: PreviewBrowserAcceptance }
	| { readonly _tag: "Missing" }
	| { readonly _tag: "Pending" };

type RenewInspectionLeaseResult = {
	readonly claim: PreviewBrowserOperationClaim;
	readonly cleanup_reason: "target_changed" | "thread_erased" | null;
};

/** Caps every active inspection ownership scope and every recovery transaction. */
export const preview_browser_active_inspection_limit = 256;

/** Distinguishes exact replay, an irreversible launch fence, and interrupted launch recovery. */
export type PreviewBrowserLaunchPreparation =
	| { readonly _tag: "Completed"; readonly acceptance: PreviewBrowserAcceptance }
	| { readonly _tag: "Pending"; readonly lease_expires_at_ms: number }
	| { readonly _tag: "Interrupted"; readonly prepared: PreparedPreviewBrowserLaunch }
	| { readonly _tag: "Prepared"; readonly prepared: PreparedPreviewBrowserLaunch };

/** Distinguishes exact replay, a new inspection fence, and interrupted attach recovery. */
export type PreviewInspectionPreparation =
	| { readonly _tag: "Completed"; readonly acceptance: PreviewBrowserAcceptance }
	| { readonly _tag: "Pending"; readonly lease_expires_at_ms: number }
	| {
			readonly _tag: "Interrupted";
			readonly prepared: Omit<PreparedPreviewInspection, "target">;
	  }
	| { readonly _tag: "Prepared"; readonly prepared: PreparedPreviewInspection };

/** Separates exact detach replay from the connector authority that must be revoked first. */
export type PreviewInspectionDetachPreparation =
	| { readonly _tag: "Completed"; readonly acceptance: PreviewBrowserAcceptance }
	| { readonly _tag: "Prepared"; readonly revocation: PreviewInspectionRevocation };

/** Describes one terminal launch result after the irreversible handoff boundary. */
export type PreviewBrowserLaunchSettlement =
	| { readonly state: "dispatched" }
	| {
			readonly reason: "launcher_rejected" | "launcher_unavailable" | "target_changed";
			readonly state: "rejected";
	  }
	| {
			readonly reason: "interrupted" | "launcher_failed";
			readonly state: "outcome_unknown";
	  };

/** Describes one terminal attach result before a live session becomes process-owned. */
export type PreviewInspectionAttachSettlement =
	| { readonly state: "attached" }
	| {
			readonly reason:
				| "connection_lost"
				| "detached"
				| "interrupted"
				| "target_changed"
				| "thread_erased";
			readonly state: "disconnected";
	  }
	| {
			readonly reason: "connector_rejected" | "connector_unavailable" | "target_changed";
			readonly state: "failed";
	  };

/** Reports a command identity or inspection identity reused with different intent. */
export class PreviewBrowserRepositoryConflict extends Data.TaggedError(
	"PreviewBrowserRepositoryConflict",
)<{
	readonly reason: "command_conflict" | "inspection_exists" | "invalid_transition";
}> {}

/** Reports a missing thread, target, or inspection session. */
export class PreviewBrowserRepositoryMissing extends Data.TaggedError(
	"PreviewBrowserRepositoryMissing",
)<{
	readonly reason: "inspection" | "target" | "thread";
	readonly subject_id: string;
}> {}

/** Reports corrupt durable browser lifecycle data. */
export class PreviewBrowserRepositoryInvariant extends Data.TaggedError(
	"PreviewBrowserRepositoryInvariant",
)<{
	readonly message: string;
}> {}

/** Reports database failures through the browser repository boundary. */
export class PreviewBrowserRepositoryStorage extends Data.TaggedError(
	"PreviewBrowserRepositoryStorage",
)<{
	readonly cause: unknown;
}> {}

/** Reports process-safe lifecycle contention without exposing private ownership. */
export class PreviewBrowserRepositoryUnavailable extends Data.TaggedError(
	"PreviewBrowserRepositoryUnavailable",
)<{
	readonly reason: "capacity" | "ownership_lost" | "target_removing" | "thread_erasing";
}> {}

export type PreviewBrowserRepositoryError =
	| PreviewBrowserRepositoryConflict
	| PreviewBrowserRepositoryInvariant
	| PreviewBrowserRepositoryMissing
	| PreviewBrowserRepositoryStorage
	| PreviewBrowserRepositoryUnavailable;

/** Owns durable launch fences, inspection projections, and canonical journal replay. */
export class PreviewBrowserRepository extends Context.Service<
	PreviewBrowserRepository,
	{
		readonly ClaimTargetRemoval: (
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewTargetRemovalClaim, PreviewBrowserRepositoryError>;
		readonly ClaimTargetRemovalFence: (
			message_id: string,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<
			Option.Option<OwnedPreviewTargetRemovalFence>,
			PreviewBrowserRepositoryError
		>;
		readonly CompleteTargetRemovalFence: (
			owned: OwnedPreviewTargetRemovalFence,
			now_ms: number,
		) => Effect.Effect<void, PreviewBrowserRepositoryError>;
		readonly RenewTargetRemoval: (
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewTargetRemovalClaim, PreviewBrowserRepositoryError>;
		readonly ActiveInspectionIdsForTargetRemoval: (
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) => Effect.Effect<ReadonlyArray<string>, PreviewBrowserRepositoryError>;
		readonly ActiveInspectionIdsForThread: (
			thread_id: string,
		) => Effect.Effect<ReadonlyArray<string>, PreviewBrowserRepositoryError>;
		readonly DetachInspection: (
			command: Command,
			now_ms: number,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserRepositoryError>;
		readonly DisconnectOwnedInspection: (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			reason: "connection_lost" | "interrupted" | "thread_erased",
			now_ms: number,
		) => Effect.Effect<Option.Option<EventEnvelope>, PreviewBrowserRepositoryError>;
		readonly DisconnectChangedInspection: (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
		) => Effect.Effect<Option.Option<EventEnvelope>, PreviewBrowserRepositoryError>;
		readonly DisconnectTargetInspection: (
			inspection_id: string,
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) => Effect.Effect<Option.Option<EventEnvelope>, PreviewBrowserRepositoryError>;
		readonly DisconnectThreadInspection: (
			inspection_id: string,
			now_ms: number,
		) => Effect.Effect<Option.Option<EventEnvelope>, PreviewBrowserRepositoryError>;
		readonly InspectionRevocation: (
			inspection_id: string,
		) => Effect.Effect<
			Option.Option<PreviewInspectionRevocation>,
			PreviewBrowserRepositoryError
		>;
		readonly List: (input: {
			readonly project_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<LifecycleQueryResult, PreviewBrowserRepositoryError>;
		readonly ListTargetRemovalFences: (input?: {
			readonly thread_id?: string;
		}) => Effect.Effect<
			ReadonlyArray<PendingPreviewTargetRemovalFence>,
			PreviewBrowserRepositoryError
		>;
		readonly ListExpiredInspectionRevocations: (
			now_ms: number,
		) => Effect.Effect<
			ReadonlyArray<PreviewInspectionRevocation>,
			PreviewBrowserRepositoryError
		>;
		readonly PrepareDetach: (
			command: Command,
		) => Effect.Effect<PreviewInspectionDetachPreparation, PreviewBrowserRepositoryError>;
		readonly PrepareInspection: (
			command: Command,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewInspectionPreparation, PreviewBrowserRepositoryError>;
		readonly PrepareLaunch: (
			command: Command,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewBrowserLaunchPreparation, PreviewBrowserRepositoryError>;
		readonly RecoverInterrupted: (
			now_ms: number,
			revoked_inspection_ids: ReadonlyArray<string>,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, PreviewBrowserRepositoryError>;
		readonly Replay: (
			command: Command,
		) => Effect.Effect<Option.Option<PreviewBrowserAcceptance>, PreviewBrowserRepositoryError>;
		readonly ReleaseTargetRemoval: (
			claim: PreviewTargetRemovalClaim,
		) => Effect.Effect<void, PreviewBrowserRepositoryError>;
		readonly RenewInspectionLease: (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<RenewInspectionLeaseResult, PreviewBrowserRepositoryError>;
		readonly RenewInspectionCleanupLease: (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
			lease_duration_ms: number,
		) => Effect.Effect<PreviewBrowserOperationClaim, PreviewBrowserRepositoryError>;
		readonly SettleInspectionAttach: (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewInspectionAttachSettlement,
			now_ms: number,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserRepositoryError>;
		readonly SettleLaunch: (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewBrowserLaunchSettlement,
			now_ms: number,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserRepositoryError>;
	}
>()("Artisan/PreviewBrowserRepository") {}

function invariant(message: string): PreviewBrowserRepositoryInvariant {
	return new PreviewBrowserRepositoryInvariant({ message });
}

function normalize_error(error: unknown): PreviewBrowserRepositoryError {
	if (
		error instanceof PreviewBrowserRepositoryConflict ||
		error instanceof PreviewBrowserRepositoryInvariant ||
		error instanceof PreviewBrowserRepositoryMissing ||
		error instanceof PreviewBrowserRepositoryUnavailable
	) {
		return error;
	}

	return new PreviewBrowserRepositoryStorage({ cause: error });
}

function lease_expiry(now_ms: number, lease_duration_ms: number) {
	if (
		!Number.isSafeInteger(now_ms) ||
		now_ms < 0 ||
		!Number.isSafeInteger(lease_duration_ms) ||
		lease_duration_ms <= 0 ||
		lease_duration_ms > 600_000 ||
		!Number.isSafeInteger(now_ms + lease_duration_ms)
	) {
		return Effect.fail(invariant("Browser lifecycle lease is invalid"));
	}

	return Effect.succeed(now_ms + lease_duration_ms);
}

function operation_claim(row: {
	readonly claim_token: string;
	readonly lease_expires_at_ms: number;
	readonly owner_instance_id: string;
}): PreviewBrowserOperationClaim {
	return {
		claim_token: row.claim_token,
		lease_expires_at_ms: row.lease_expires_at_ms,
		owner_instance_id: row.owner_instance_id,
	};
}

function is_browser_command(command: Command): command is BrowserCommand {
	return (
		command.payload.type === "preview.browser.open" ||
		command.payload.type === "preview.inspection.attach" ||
		command.payload.type === "preview.inspection.detach"
	);
}

function is_launch_command(command: Command): command is BrowserCommand & {
	readonly payload: Extract<BrowserPayload, { readonly type: "preview.browser.open" }>;
} {
	return command.payload.type === "preview.browser.open";
}

function is_attach_command(command: Command): command is BrowserCommand & {
	readonly payload: Extract<BrowserPayload, { readonly type: "preview.inspection.attach" }>;
} {
	return command.payload.type === "preview.inspection.attach";
}

function is_detach_command(command: Command): command is BrowserCommand & {
	readonly payload: Extract<BrowserPayload, { readonly type: "preview.inspection.detach" }>;
} {
	return command.payload.type === "preview.inspection.detach";
}

function command_matches(
	command: BrowserCommand,
	encoded: EncodedCommand,
	existing: typeof JournalCommands.$inferSelect,
) {
	return (
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.payload_json === encoded.payload_json &&
		existing.payload_type === command.payload.type &&
		existing.raw_origin_json === encoded.raw_origin_json &&
		existing.run_id === (command.run_id ?? null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at &&
		existing.thread_id === command.thread_id
	);
}

function initiator_matches(
	initiator: PreviewBrowserInitiator,
	kind: string,
	agent_id: string | null,
) {
	return initiator.kind === "user"
		? kind === "user" && agent_id === null
		: kind === "agent" && agent_id === initiator.agent_id;
}

/** Keeps internal disconnect identity outside the protocol Identifier domain. */
function inspection_disconnect_idempotency_key(inspection_id: string) {
	return `preview.inspection.disconnect ${inspection_id}`;
}

const EncodeCommandEnvelopeJson = Schema.encodeEffect(Schema.fromJsonString(CommandEnvelope), {
	onExcessProperty: "error",
});
const DecodeCommandEnvelopeJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(CommandEnvelope),
	{ onExcessProperty: "error" },
);
const DecodeCommandPayloadJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(CommandEnvelope.fields.payload),
	{ onExcessProperty: "error" },
);
const EncodeCommandPayloadJson = Schema.encodeEffect(
	Schema.fromJsonString(CommandEnvelope.fields.payload),
	{ onExcessProperty: "error" },
);
const EncodeRawOriginJson = Schema.encodeEffect(Schema.fromJsonString(RawOrigin), {
	onExcessProperty: "error",
});
const DecodeRawOriginJson = Schema.decodeUnknownEffect(Schema.fromJsonString(RawOrigin), {
	onExcessProperty: "error",
});
const EncodeLifecycleEventJson = Schema.encodeEffect(
	Schema.fromJsonString(PreviewBrowserLifecycleEvent),
	{ onExcessProperty: "error" },
);
const DecodeLifecycleEventJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(PreviewBrowserLifecycleEvent),
	{ onExcessProperty: "error" },
);
const DecodePreviewTargetEventJson = Schema.decodeUnknownEffect(
	Schema.fromJsonString(PreviewTargetUpdatedEvent),
	{ onExcessProperty: "error" },
);

const EncodeCommand = (command: BrowserCommand) =>
	Effect.gen(function* () {
		const command_json = yield* EncodeCommandEnvelopeJson(command);
		const payload_json = yield* EncodeCommandPayloadJson(command.payload);
		const raw_origin_json =
			command.raw_origin === undefined
				? null
				: yield* EncodeRawOriginJson(command.raw_origin);

		return { command_json, payload_json, raw_origin_json } satisfies EncodedCommand;
	}).pipe(
		Effect.mapError(() => invariant("Browser lifecycle command cannot be encoded canonically")),
	);

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
		} satisfies PreviewTargetHealth;
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

const DecodeLaunch = (row: LaunchRow) =>
	Schema.decodeUnknownEffect(PreviewBrowserLaunchRecord, {
		onExcessProperty: "error",
	})({
		initiator:
			row.initiator_kind === "agent" && row.initiator_agent_id !== null
				? { agent_id: row.initiator_agent_id, kind: "agent" }
				: { kind: "user" },
		launch_id: row.message_id,
		project_id: row.project_id,
		...(row.reason === null ? {} : { reason: row.reason }),
		requested_at_ms: row.requested_at_ms,
		state: row.state,
		target_generation_id: row.target_generation_id,
		target_id: row.target_id,
		updated_at_ms: row.updated_at_ms,
		url: row.url,
		workspace_id: row.workspace_id,
	}).pipe(Effect.mapError(() => invariant("Stored browser launch is corrupt")));

const DecodeInspection = (row: InspectionRow) =>
	Schema.decodeUnknownEffect(PreviewInspectionSessionRecord, {
		onExcessProperty: "error",
	})({
		connector_id: row.connector_id,
		initiator:
			row.initiator_kind === "agent" && row.initiator_agent_id !== null
				? { agent_id: row.initiator_agent_id, kind: "agent" }
				: { kind: "user" },
		inspection_id: row.inspection_id,
		project_id: row.project_id,
		...(row.reason === null ? {} : { reason: row.reason }),
		requested_at_ms: row.requested_at_ms,
		state: row.state,
		target_generation_id: row.target_generation_id,
		target_id: row.target_id,
		updated_at_ms: row.updated_at_ms,
		url: row.url,
		workspace_id: row.workspace_id,
	}).pipe(Effect.mapError(() => invariant("Stored inspection session is corrupt")));

const DecodeTargetRemovalFence = (row: TargetRemovalFenceRow) =>
	Effect.gen(function* () {
		if (!Number.isSafeInteger(row.committed_at_ms) || row.committed_at_ms < 0) {
			return yield* invariant("Stored target-removal fence timestamp is corrupt");
		}

		return {
			committed_at_ms: row.committed_at_ms,
			message_id: row.message_id,
			project_id: row.project_id,
			target_generation_id: row.target_generation_id,
			target_id: row.target_id,
			thread_id: row.thread_id,
			workspace_id: row.workspace_id,
		} satisfies PendingPreviewTargetRemovalFence;
	});

/** Provides SQLite-backed browser lifecycle fencing and exact public replay. */
export const PreviewBrowserRepositoryLive = Layer.effect(
	PreviewBrowserRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const MakeOperationClaim = (now_ms: number, lease_duration_ms: number) =>
			Effect.gen(function* () {
				const lease_expires_at_ms = yield* lease_expiry(now_ms, lease_duration_ms);
				const claim_token = yield* metadata.MakeId("claim");

				return {
					claim_token,
					lease_expires_at_ms,
					owner_instance_id: metadata.instance_id,
				} satisfies PreviewBrowserOperationClaim;
			});

		const DecodeEvent = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* DecodeLifecycleEventJson(row.payload_json);
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
				}).pipe(
					Effect.mapError(() => invariant("Stored browser lifecycle event is corrupt")),
				);
			});
		const AttestTargetRemovalFence = (
			transaction: typeof database.client,
			fence: PendingPreviewTargetRemovalFence,
		) =>
			Effect.gen(function* () {
				const [command_row] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, fence.message_id))
					.limit(1);
				const [event_row] = yield* transaction
					.select()
					.from(JournalEvents)
					.where(
						and(
							eq(JournalEvents.idempotency_key, fence.message_id),
							eq(JournalEvents.event_type, "preview.target.updated"),
						),
					)
					.limit(1);

				if (!command_row || !event_row) {
					return yield* invariant(
						"Target-removal fence has no canonical journal evidence",
					);
				}

				const command_payload = yield* DecodeCommandPayloadJson(
					command_row.payload_json,
				).pipe(
					Effect.mapError(() => invariant("Stored target-removal command is corrupt")),
				);
				const event_payload = yield* DecodePreviewTargetEventJson(
					event_row.payload_json,
				).pipe(Effect.mapError(() => invariant("Stored target-removal event is corrupt")));
				const event_generation_id =
					event_payload.action === "removed" && "generation_id" in event_payload.target
						? event_payload.target.generation_id
						: undefined;

				if (
					command_row.status !== "accepted" ||
					command_row.thread_id !== fence.thread_id ||
					command_payload.type !== "preview.target.remove" ||
					command_payload.project_id !== fence.project_id ||
					command_payload.workspace_id !== fence.workspace_id ||
					command_payload.target_id !== fence.target_id ||
					event_row.causation_id !== fence.message_id ||
					event_row.correlation_id !== fence.message_id ||
					event_row.thread_id !== fence.thread_id ||
					event_payload.action !== "removed" ||
					event_payload.target.project_id !== fence.project_id ||
					event_payload.target.workspace_id !== fence.workspace_id ||
					event_payload.target.target_id !== fence.target_id ||
					event_generation_id !== fence.target_generation_id ||
					event_payload.target.updated_at_ms !== fence.committed_at_ms
				) {
					return yield* invariant(
						"Target-removal fence journal evidence is inconsistent",
					);
				}
			});

		const ReadOptionalEvent = (transaction: typeof database.client, idempotency_key: string) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.idempotency_key, idempotency_key),
						inArray(JournalEvents.event_type, [
							"preview.browser.launch.updated",
							"preview.inspection.updated",
						]),
					),
				)
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEvent(row).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none()),
					),
				);
		const ReadThreadStream = (
			transaction: typeof database.client,
			thread_id: string,
			require_unclaimed: boolean,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${thread_id}`;
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [claim] = require_unclaimed
					? yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, thread_id))
							.limit(1)
					: [];
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* new PreviewBrowserRepositoryMissing({
						reason: "thread",
						subject_id: thread_id,
					});
				}

				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);

				if (!stream) {
					return yield* new PreviewBrowserRepositoryMissing({
						reason: "thread",
						subject_id: thread_id,
					});
				}

				return { last_sequence: stream.last_sequence, stream_id };
			});

		const ReadTarget = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
		) =>
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
					return yield* new PreviewBrowserRepositoryMissing({
						reason: "target",
						subject_id: input.target_id,
					});
				}

				return { generation_id: row.generation_id, target: yield* DecodeTarget(row) };
			});

		const TargetRemovalIsLive = (
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

				return claim !== undefined;
			});
		const TargetGenerationRemovalIsPending = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_generation_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
		) =>
			Effect.gen(function* () {
				const [fence] = yield* transaction
					.select({ message_id: PreviewTargetRemovalFences.message_id })
					.from(PreviewTargetRemovalFences)
					.where(
						and(
							eq(PreviewTargetRemovalFences.project_id, input.project_id),
							eq(PreviewTargetRemovalFences.workspace_id, input.workspace_id),
							eq(PreviewTargetRemovalFences.target_id, input.target_id),
							eq(
								PreviewTargetRemovalFences.target_generation_id,
								input.target_generation_id,
							),
						),
					)
					.limit(1);

				return fence !== undefined;
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
				const target_removing = yield* TargetRemovalIsLive(transaction, input, now_ms);

				if (target_removing) {
					return yield* new PreviewBrowserRepositoryUnavailable({
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
					return yield* new PreviewBrowserRepositoryUnavailable({
						reason: "ownership_lost",
					});
				}
			});
		const EnsureTargetGenerationCurrent = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_generation_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
		) =>
			Effect.gen(function* () {
				const target_is_current = yield* TargetGenerationIsCurrent(transaction, input);

				if (!target_is_current) {
					return yield* new PreviewBrowserRepositoryUnavailable({
						reason: "target_removing",
					});
				}
			});
		const TargetGenerationIsCurrent = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_generation_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
		) =>
			Effect.gen(function* () {
				const [target] = yield* transaction
					.select({ generation_id: PreviewTargets.generation_id })
					.from(PreviewTargets)
					.where(
						and(
							eq(PreviewTargets.project_id, input.project_id),
							eq(PreviewTargets.workspace_id, input.workspace_id),
							eq(PreviewTargets.target_id, input.target_id),
						),
					)
					.limit(1);

				return target?.generation_id === input.target_generation_id;
			});
		const EnsureOperationTargetCurrent = (
			transaction: typeof database.client,
			input: {
				readonly project_id: string;
				readonly target_generation_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
			now_ms: number,
		) =>
			Effect.gen(function* () {
				yield* EnsureTargetNotRemoving(transaction, input, now_ms);
				yield* EnsureTargetGenerationCurrent(transaction, input);
			});
		const ReadActiveOwnedInspection = (
			transaction: typeof database.client,
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
		) =>
			Effect.gen(function* () {
				const [owned] = yield* transaction
					.select({
						project_id: PreviewInspectionSessions.project_id,
						target_generation_id: PreviewInspectionSessions.target_generation_id,
						target_id: PreviewInspectionSessions.target_id,
						thread_id: PreviewInspectionSessions.thread_id,
						workspace_id: PreviewInspectionSessions.workspace_id,
					})
					.from(PreviewInspectionSessions)
					.where(
						and(
							eq(PreviewInspectionSessions.inspection_id, inspection_id),
							eq(PreviewInspectionSessions.claim_token, claim.claim_token),
							eq(
								PreviewInspectionSessions.owner_instance_id,
								claim.owner_instance_id,
							),
							eq(
								PreviewInspectionSessions.lease_expires_at_ms,
								claim.lease_expires_at_ms,
							),
							gt(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
							eq(PreviewInspectionSessions.state, "attached"),
						),
					)
					.limit(1);

				if (!owned) {
					return yield* new PreviewBrowserRepositoryUnavailable({
						reason: "ownership_lost",
					});
				}

				return owned;
			});
		const RenewActiveInspectionLease = (
			transaction: typeof database.client,
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
			lease_expires_at_ms: number,
		) =>
			Effect.gen(function* () {
				const [renewed] = yield* transaction
					.update(PreviewInspectionSessions)
					.set({ lease_expires_at_ms })
					.where(
						and(
							eq(PreviewInspectionSessions.inspection_id, inspection_id),
							eq(PreviewInspectionSessions.claim_token, claim.claim_token),
							eq(
								PreviewInspectionSessions.owner_instance_id,
								claim.owner_instance_id,
							),
							eq(
								PreviewInspectionSessions.lease_expires_at_ms,
								claim.lease_expires_at_ms,
							),
							gt(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
							eq(PreviewInspectionSessions.state, "attached"),
						),
					)
					.returning({
						claim_token: PreviewInspectionSessions.claim_token,
						lease_expires_at_ms: PreviewInspectionSessions.lease_expires_at_ms,
						owner_instance_id: PreviewInspectionSessions.owner_instance_id,
					});

				if (!renewed) {
					return yield* new PreviewBrowserRepositoryUnavailable({
						reason: "ownership_lost",
					});
				}

				return operation_claim(renewed);
			});

		const ReplayTransaction = (
			transaction: typeof database.client,
			command: BrowserCommand,
			encoded: EncodedCommand,
		) =>
			Effect.gen(function* () {
				const [existing] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, command.message_id))
					.limit(1);

				if (!existing) {
					return { _tag: "Missing" as const } satisfies BrowserCommandReplay;
				}

				if (!command_matches(command, encoded, existing)) {
					return yield* new PreviewBrowserRepositoryConflict({
						reason: "command_conflict",
					});
				}

				const event = yield* ReadOptionalEvent(transaction, command.message_id);

				if (Option.isSome(event)) {
					return {
						_tag: "Completed" as const,
						acceptance: { event: event.value, status: "duplicate" as const },
					} satisfies BrowserCommandReplay;
				}

				if (existing.status === "pending") {
					return { _tag: "Pending" as const } satisfies BrowserCommandReplay;
				}

				return yield* invariant("Browser lifecycle command event is missing");
			});

		const ValidateLaunchIntent = (
			row: LaunchRow,
			command: BrowserCommand & {
				readonly payload: Extract<
					BrowserPayload,
					{ readonly type: "preview.browser.open" }
				>;
			},
			encoded: EncodedCommand,
		) =>
			Effect.gen(function* () {
				const initiator = preview_browser_initiator(command);

				if (
					row.command_json !== encoded.command_json ||
					row.message_id !== command.message_id ||
					row.thread_id !== command.thread_id ||
					row.project_id !== command.payload.project_id ||
					row.workspace_id !== command.payload.workspace_id ||
					row.target_id !== command.payload.target_id ||
					!initiator_matches(initiator, row.initiator_kind, row.initiator_agent_id)
				) {
					return yield* new PreviewBrowserRepositoryConflict({
						reason: "command_conflict",
					});
				}
			});

		const ValidateInspectionIntent = (
			row: InspectionRow,
			command: BrowserCommand & {
				readonly payload: Extract<
					BrowserPayload,
					{ readonly type: "preview.inspection.attach" }
				>;
			},
			encoded: EncodedCommand,
		) =>
			Effect.gen(function* () {
				const initiator = preview_browser_initiator(command);

				if (
					row.attach_command_json !== encoded.command_json ||
					row.attach_message_id !== command.message_id ||
					row.inspection_id !== command.payload.inspection_id ||
					row.thread_id !== command.thread_id ||
					row.project_id !== command.payload.project_id ||
					row.workspace_id !== command.payload.workspace_id ||
					row.target_id !== command.payload.target_id ||
					row.connector_id !== command.payload.connector_id ||
					!initiator_matches(initiator, row.initiator_kind, row.initiator_agent_id)
				) {
					return yield* new PreviewBrowserRepositoryConflict({
						reason: "command_conflict",
					});
				}
			});

		const AppendEvent = (
			transaction: typeof database.client,
			command: BrowserCommand,
			payload: LifecycleEvent,
			idempotency_key: string,
		) =>
			Effect.gen(function* () {
				const payload_json = yield* EncodeLifecycleEventJson(payload).pipe(
					Effect.mapError(() => invariant("Browser lifecycle event cannot be encoded")),
				);
				const raw_origin_json =
					command.raw_origin === undefined
						? null
						: yield* EncodeRawOriginJson(command.raw_origin).pipe(
								Effect.mapError(() =>
									invariant("Browser event origin cannot be encoded"),
								),
							);
				const existing = yield* transaction
					.select()
					.from(JournalEvents)
					.where(eq(JournalEvents.idempotency_key, idempotency_key))
					.limit(1);

				if (existing[0]) {
					const row = existing[0];

					if (
						row.agent_id !== (command.agent_id ?? null) ||
						row.causation_id !== command.message_id ||
						row.correlation_id !== command.message_id ||
						row.event_type !== payload.type ||
						row.origin !== "backend" ||
						row.payload_json !== payload_json ||
						row.raw_origin_json !== raw_origin_json ||
						row.run_id !== (command.run_id ?? null) ||
						row.schema_version !== 1 ||
						row.stream_id !== `thread:${command.thread_id}` ||
						row.thread_id !== command.thread_id
					) {
						return yield* invariant("Browser lifecycle idempotency key collided");
					}

					return yield* DecodeEvent(row);
				}

				const stream = yield* ReadThreadStream(transaction, command.thread_id, false);
				const occurred_at = yield* metadata.Now;
				const sequence = stream.last_sequence + 1;
				const event_id = yield* metadata.MakeId("event");

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
						idempotency_key,
						occurred_at,
						origin: "backend",
						payload_json,
						raw_origin_json,
						run_id: command.run_id ?? null,
						schema_version: 1,
						stream_id: stream.stream_id,
						stream_sequence: sequence,
						thread_id: command.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				if (!inserted) {
					return yield* invariant("Browser lifecycle event was not persisted");
				}

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					...(command.agent_id === undefined ? {} : { agent_id: command.agent_id }),
					causation_id: command.message_id,
					correlation_id: command.message_id,
					journal_sequence: inserted.journal_sequence,
					kind: "event",
					message_id: event_id,
					origin: "backend",
					payload,
					protocol_version: 1,
					...(command.raw_origin === undefined ? {} : { raw_origin: command.raw_origin }),
					...(command.run_id === undefined ? {} : { run_id: command.run_id }),
					schema_version: 1,
					sequence,
					sent_at: occurred_at,
					stream_id: stream.stream_id,
					thread_id: command.thread_id,
				}).pipe(
					Effect.mapError(() => invariant("Browser lifecycle event envelope is invalid")),
				);
			});

		const InsertCommand = (
			transaction: typeof database.client,
			command: BrowserCommand,
			encoded: EncodedCommand,
			status: "accepted" | "pending",
		) =>
			Effect.gen(function* () {
				const accepted_at = yield* metadata.Now;

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
					status,
					thread_id: command.thread_id,
				});
			});

		const Accept = (
			command: BrowserCommand,
			mutate: (
				transaction: typeof database.client,
				encoded: EncodedCommand,
			) => Effect.Effect<
				{
					readonly payload: LifecycleEvent;
					readonly preceding_events: ReadonlyArray<EventEnvelope>;
				},
				unknown
			>,
		) =>
			Effect.gen(function* () {
				const encoded = yield* EncodeCommand(command);
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (replayed._tag === "Completed") {
								return {
									_tag: "Duplicate" as const,
									acceptance: replayed.acceptance,
								};
							}

							if (replayed._tag === "Pending") {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "ownership_lost",
								});
							}

							const mutation = yield* mutate(transaction, encoded);

							yield* InsertCommand(transaction, command, encoded, "accepted");

							const event = yield* AppendEvent(
								transaction,
								command,
								mutation.payload,
								command.message_id,
							);

							return {
								_tag: "Accepted" as const,
								event,
								preceding_events: mutation.preceding_events,
							};
						}),
					),
				);

				if (result._tag === "Duplicate") {
					return result.acceptance;
				}

				yield* Effect.forEach(
					[...result.preceding_events, result.event],
					(event) => notifier.Publish(event.journal_sequence),
					{ discard: true },
				);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		const AcceptReserved = (
			command: BrowserCommand,
			mutate: (
				transaction: typeof database.client,
				encoded: EncodedCommand,
			) => Effect.Effect<LifecycleEvent, unknown>,
		) =>
			Effect.gen(function* () {
				const encoded = yield* EncodeCommand(command);
				const result = yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (replayed._tag === "Completed") {
								return {
									_tag: "Duplicate" as const,
									acceptance: replayed.acceptance,
								};
							}

							if (replayed._tag === "Missing") {
								return yield* invariant(
									"Prepared browser operation lost its command reservation",
								);
							}

							const payload = yield* mutate(transaction, encoded);

							yield* transaction
								.update(JournalCommands)
								.set({ status: "accepted" })
								.where(eq(JournalCommands.message_id, command.message_id));

							const event = yield* AppendEvent(
								transaction,
								command,
								payload,
								command.message_id,
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

		const Replay = (command: Command) =>
			Effect.gen(function* () {
				if (!is_browser_command(command)) {
					return yield* invariant("Browser repository received a non-browser command");
				}

				const encoded = yield* EncodeCommand(command);

				const replayed = yield* database.client.transaction((transaction) =>
					ReplayTransaction(transaction, command, encoded),
				);

				return replayed._tag === "Completed"
					? Option.some(replayed.acceptance)
					: Option.none<PreviewBrowserAcceptance>();
			}).pipe(Effect.mapError(normalize_error));

		const PrepareLaunch = (command: Command, now_ms: number, lease_duration_ms: number) =>
			Effect.gen(function* () {
				if (!is_launch_command(command)) {
					return yield* invariant("Browser launch preparation requires an open command");
				}

				const encoded = yield* EncodeCommand(command);
				const initiator = preview_browser_initiator(command);

				yield* lease_expiry(now_ms, lease_duration_ms);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ReadThreadStream(transaction, command.thread_id, true);
							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (replayed._tag === "Completed") {
								return {
									_tag: "Completed" as const,
									acceptance: replayed.acceptance,
								};
							}

							const [existing] = yield* transaction
								.select()
								.from(PreviewBrowserLaunches)
								.where(eq(PreviewBrowserLaunches.message_id, command.message_id))
								.limit(1);

							if (existing) {
								if (replayed._tag === "Missing") {
									return yield* invariant(
										"Browser launch lost its command reservation",
									);
								}

								yield* ValidateLaunchIntent(existing, command, encoded);
								yield* EnsureOperationTargetCurrent(transaction, existing, now_ms);

								if (
									existing.state !== "accepted" &&
									existing.state !== "dispatching"
								) {
									return yield* invariant(
										"Terminal browser launch is missing its journal event",
									);
								}

								if (existing.lease_expires_at_ms > now_ms) {
									return {
										_tag: "Pending" as const,
										lease_expires_at_ms: existing.lease_expires_at_ms,
									};
								}

								const claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);
								const [reclaimed] = yield* transaction
									.update(PreviewBrowserLaunches)
									.set(claim)
									.where(
										and(
											eq(
												PreviewBrowserLaunches.message_id,
												command.message_id,
											),
											eq(
												PreviewBrowserLaunches.claim_token,
												existing.claim_token,
											),
											eq(
												PreviewBrowserLaunches.owner_instance_id,
												existing.owner_instance_id,
											),
											eq(
												PreviewBrowserLaunches.lease_expires_at_ms,
												existing.lease_expires_at_ms,
											),
											inArray(PreviewBrowserLaunches.state, [
												"accepted",
												"dispatching",
											]),
										),
									)
									.returning();

								if (!reclaimed) {
									return {
										_tag: "Pending" as const,
										lease_expires_at_ms: existing.lease_expires_at_ms,
									};
								}

								return {
									_tag: "Interrupted" as const,
									prepared: {
										claim,
										command,
										launch: yield* DecodeLaunch(reclaimed),
									},
								};
							}

							if (replayed._tag === "Pending") {
								return yield* invariant(
									"Reserved browser launch lost its durable operation",
								);
							}

							yield* EnsureTargetNotRemoving(transaction, command.payload, now_ms);

							const target = yield* ReadTarget(transaction, command.payload);
							const claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);
							const launch = yield* Schema.decodeUnknownEffect(
								PreviewBrowserLaunchRecord,
								{ onExcessProperty: "error" },
							)({
								initiator,
								launch_id: command.message_id,
								project_id: command.payload.project_id,
								requested_at_ms: now_ms,
								state: "dispatching",
								target_generation_id: target.generation_id,
								target_id: command.payload.target_id,
								updated_at_ms: now_ms,
								url: target.target.url,
								workspace_id: command.payload.workspace_id,
							}).pipe(
								Effect.mapError(() =>
									invariant("Prepared browser launch is invalid"),
								),
							);

							yield* InsertCommand(transaction, command, encoded, "pending");
							yield* transaction.insert(PreviewBrowserLaunches).values({
								...claim,
								command_json: encoded.command_json,
								initiator_agent_id:
									initiator.kind === "agent" ? initiator.agent_id : null,
								initiator_kind: initiator.kind,
								message_id: command.message_id,
								project_id: launch.project_id,
								reason: null,
								requested_at_ms: launch.requested_at_ms,
								state: launch.state,
								target_generation_id: launch.target_generation_id,
								target_id: launch.target_id,
								thread_id: command.thread_id,
								updated_at_ms: launch.updated_at_ms,
								url: launch.url,
								workspace_id: launch.workspace_id,
							});

							return {
								_tag: "Prepared" as const,
								prepared: { claim, command, launch },
							};
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));

		const SettleLaunchInternal = (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewBrowserLaunchSettlement,
			now_ms: number,
			lease_state: "active" | "expired",
		) => {
			if (!is_launch_command(command)) {
				return Effect.fail(invariant("Browser launch settlement requires an open command"));
			}

			return AcceptReserved(command, (transaction, encoded) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(PreviewBrowserLaunches)
						.where(eq(PreviewBrowserLaunches.message_id, command.message_id))
						.limit(1);

					if (!row) {
						return yield* new PreviewBrowserRepositoryMissing({
							reason: "target",
							subject_id: command.payload.target_id,
						});
					}

					yield* ValidateLaunchIntent(row, command, encoded);

					if (lease_state === "active") {
						yield* EnsureOperationTargetCurrent(transaction, row, now_ms);
					}

					if (row.state !== "accepted" && row.state !== "dispatching") {
						return yield* new PreviewBrowserRepositoryConflict({
							reason: "invalid_transition",
						});
					}

					const updated_at_ms = Math.max(now_ms, row.requested_at_ms);
					const [updated] = yield* transaction
						.update(PreviewBrowserLaunches)
						.set({
							reason: settlement.state === "dispatched" ? null : settlement.reason,
							state: settlement.state,
							updated_at_ms,
						})
						.where(
							and(
								eq(PreviewBrowserLaunches.message_id, command.message_id),
								eq(PreviewBrowserLaunches.claim_token, claim.claim_token),
								eq(
									PreviewBrowserLaunches.owner_instance_id,
									claim.owner_instance_id,
								),
								eq(
									PreviewBrowserLaunches.lease_expires_at_ms,
									claim.lease_expires_at_ms,
								),
								...(lease_state === "active"
									? [gt(PreviewBrowserLaunches.lease_expires_at_ms, now_ms)]
									: []),
								inArray(PreviewBrowserLaunches.state, ["accepted", "dispatching"]),
							),
						)
						.returning();

					if (!updated) {
						return yield* new PreviewBrowserRepositoryUnavailable({
							reason: "ownership_lost",
						});
					}

					const launch = yield* DecodeLaunch(updated);

					return yield* Schema.decodeUnknownEffect(PreviewBrowserLifecycleEvent, {
						onExcessProperty: "error",
					})({
						action: settlement.state,
						launch,
						type: "preview.browser.launch.updated",
					}).pipe(Effect.mapError(() => invariant("Browser launch event is invalid")));
				}),
			);
		};
		const SettleLaunch = (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewBrowserLaunchSettlement,
			now_ms: number,
		) => SettleLaunchInternal(command, claim, settlement, now_ms, "active");

		const PrepareInspection = (command: Command, now_ms: number, lease_duration_ms: number) =>
			Effect.gen(function* () {
				if (!is_attach_command(command)) {
					return yield* invariant("Inspection preparation requires an attach command");
				}

				const encoded = yield* EncodeCommand(command);
				const initiator = preview_browser_initiator(command);

				yield* lease_expiry(now_ms, lease_duration_ms);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ReadThreadStream(transaction, command.thread_id, true);
							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (replayed._tag === "Completed") {
								return {
									_tag: "Completed" as const,
									acceptance: replayed.acceptance,
								};
							}

							const [existing] = yield* transaction
								.select()
								.from(PreviewInspectionSessions)
								.where(
									eq(
										PreviewInspectionSessions.inspection_id,
										command.payload.inspection_id,
									),
								)
								.limit(1);

							if (existing) {
								if (existing.attach_message_id !== command.message_id) {
									return yield* new PreviewBrowserRepositoryConflict({
										reason: "inspection_exists",
									});
								}

								if (replayed._tag === "Missing") {
									return yield* invariant(
										"Inspection attach lost its command reservation",
									);
								}

								yield* ValidateInspectionIntent(existing, command, encoded);
								yield* EnsureOperationTargetCurrent(transaction, existing, now_ms);

								if (existing.state !== "attaching") {
									return yield* invariant(
										"Settled inspection is missing its journal event",
									);
								}

								if (existing.lease_expires_at_ms > now_ms) {
									return {
										_tag: "Pending" as const,
										lease_expires_at_ms: existing.lease_expires_at_ms,
									};
								}

								const claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);
								const [reclaimed] = yield* transaction
									.update(PreviewInspectionSessions)
									.set(claim)
									.where(
										and(
											eq(
												PreviewInspectionSessions.inspection_id,
												command.payload.inspection_id,
											),
											eq(
												PreviewInspectionSessions.claim_token,
												existing.claim_token,
											),
											eq(
												PreviewInspectionSessions.owner_instance_id,
												existing.owner_instance_id,
											),
											eq(
												PreviewInspectionSessions.lease_expires_at_ms,
												existing.lease_expires_at_ms,
											),
											eq(PreviewInspectionSessions.state, "attaching"),
										),
									)
									.returning();

								if (!reclaimed) {
									return {
										_tag: "Pending" as const,
										lease_expires_at_ms: existing.lease_expires_at_ms,
									};
								}

								return {
									_tag: "Interrupted" as const,
									prepared: {
										claim,
										command,
										inspection: yield* DecodeInspection(reclaimed),
									},
								};
							}

							if (replayed._tag === "Pending") {
								return yield* invariant(
									"Reserved inspection attach lost its durable operation",
								);
							}

							yield* EnsureTargetNotRemoving(transaction, command.payload, now_ms);

							const thread_rows = yield* transaction
								.select({ inspection_id: PreviewInspectionSessions.inspection_id })
								.from(PreviewInspectionSessions)
								.where(
									and(
										eq(PreviewInspectionSessions.thread_id, command.thread_id),
										inArray(PreviewInspectionSessions.state, [
											"attached",
											"attaching",
										]),
									),
								)
								.orderBy(asc(PreviewInspectionSessions.inspection_id))
								.limit(preview_browser_active_inspection_limit);
							const target_rows = yield* transaction
								.select({ inspection_id: PreviewInspectionSessions.inspection_id })
								.from(PreviewInspectionSessions)
								.where(
									and(
										eq(
											PreviewInspectionSessions.project_id,
											command.payload.project_id,
										),
										eq(
											PreviewInspectionSessions.workspace_id,
											command.payload.workspace_id,
										),
										eq(
											PreviewInspectionSessions.target_id,
											command.payload.target_id,
										),
										inArray(PreviewInspectionSessions.state, [
											"attached",
											"attaching",
										]),
									),
								)
								.orderBy(asc(PreviewInspectionSessions.inspection_id))
								.limit(preview_browser_active_inspection_limit);

							if (
								thread_rows.length >= preview_browser_active_inspection_limit ||
								target_rows.length >= preview_browser_active_inspection_limit
							) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "capacity",
								});
							}

							const target = yield* ReadTarget(transaction, command.payload);
							const claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);
							const inspection = yield* Schema.decodeUnknownEffect(
								PreviewInspectionSessionRecord,
								{ onExcessProperty: "error" },
							)({
								connector_id: command.payload.connector_id,
								initiator,
								inspection_id: command.payload.inspection_id,
								project_id: command.payload.project_id,
								requested_at_ms: now_ms,
								state: "attaching",
								target_generation_id: target.generation_id,
								target_id: command.payload.target_id,
								updated_at_ms: now_ms,
								url: target.target.url,
								workspace_id: command.payload.workspace_id,
							}).pipe(
								Effect.mapError(() => invariant("Prepared inspection is invalid")),
							);

							yield* InsertCommand(transaction, command, encoded, "pending");
							yield* transaction.insert(PreviewInspectionSessions).values({
								...claim,
								attach_command_json: encoded.command_json,
								attach_message_id: command.message_id,
								connector_id: inspection.connector_id,
								initiator_agent_id:
									initiator.kind === "agent" ? initiator.agent_id : null,
								initiator_kind: initiator.kind,
								inspection_id: inspection.inspection_id,
								project_id: inspection.project_id,
								reason: null,
								requested_at_ms: inspection.requested_at_ms,
								state: inspection.state,
								target_generation_id: inspection.target_generation_id,
								target_id: inspection.target_id,
								thread_id: command.thread_id,
								updated_at_ms: inspection.updated_at_ms,
								url: inspection.url,
								workspace_id: inspection.workspace_id,
							});

							return {
								_tag: "Prepared" as const,
								prepared: { claim, command, inspection, target: target.target },
							};
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));

		const SettleInspectionAttachInternal = (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewInspectionAttachSettlement,
			now_ms: number,
			lease_state: "active" | "expired",
		) => {
			if (!is_attach_command(command)) {
				return Effect.fail(invariant("Inspection settlement requires an attach command"));
			}

			return AcceptReserved(command, (transaction, encoded) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(PreviewInspectionSessions)
						.where(
							eq(
								PreviewInspectionSessions.inspection_id,
								command.payload.inspection_id,
							),
						)
						.limit(1);

					if (!row) {
						return yield* new PreviewBrowserRepositoryMissing({
							reason: "inspection",
							subject_id: command.payload.inspection_id,
						});
					}

					yield* ValidateInspectionIntent(row, command, encoded);

					if (lease_state === "active") {
						yield* EnsureOperationTargetCurrent(transaction, row, now_ms);
					}

					if (row.state !== "attaching") {
						return yield* new PreviewBrowserRepositoryConflict({
							reason: "invalid_transition",
						});
					}

					const updated_at_ms = Math.max(now_ms, row.requested_at_ms);
					const [updated] = yield* transaction
						.update(PreviewInspectionSessions)
						.set({
							reason: settlement.state === "attached" ? null : settlement.reason,
							state: settlement.state,
							updated_at_ms,
						})
						.where(
							and(
								eq(
									PreviewInspectionSessions.inspection_id,
									command.payload.inspection_id,
								),
								eq(PreviewInspectionSessions.claim_token, claim.claim_token),
								eq(
									PreviewInspectionSessions.owner_instance_id,
									claim.owner_instance_id,
								),
								eq(
									PreviewInspectionSessions.lease_expires_at_ms,
									claim.lease_expires_at_ms,
								),
								...(lease_state === "active"
									? [gt(PreviewInspectionSessions.lease_expires_at_ms, now_ms)]
									: []),
								eq(PreviewInspectionSessions.state, "attaching"),
							),
						)
						.returning();

					if (!updated) {
						return yield* new PreviewBrowserRepositoryUnavailable({
							reason: "ownership_lost",
						});
					}

					const inspection = yield* DecodeInspection(updated);

					return yield* Schema.decodeUnknownEffect(PreviewBrowserLifecycleEvent, {
						onExcessProperty: "error",
					})({
						action: settlement.state,
						inspection,
						type: "preview.inspection.updated",
					}).pipe(Effect.mapError(() => invariant("Inspection attach event is invalid")));
				}),
			);
		};
		const SettleInspectionAttach = (
			command: Command,
			claim: PreviewBrowserOperationClaim,
			settlement: PreviewInspectionAttachSettlement,
			now_ms: number,
		) => SettleInspectionAttachInternal(command, claim, settlement, now_ms, "active");
		const InspectionRevocation = (inspection_id: string) =>
			database.client
				.select({
					connector_id: PreviewInspectionSessions.connector_id,
					inspection_id: PreviewInspectionSessions.inspection_id,
				})
				.from(PreviewInspectionSessions)
				.where(
					and(
						eq(PreviewInspectionSessions.inspection_id, inspection_id),
						inArray(PreviewInspectionSessions.state, ["attached", "attaching"]),
					),
				)
				.limit(1)
				.pipe(
					Effect.map(([row]) =>
						row
							? Option.some(row satisfies PreviewInspectionRevocation)
							: Option.none<PreviewInspectionRevocation>(),
					),
					Effect.mapError(normalize_error),
				);
		const ListExpiredInspectionRevocations = (now_ms: number) =>
			database.client
				.select({
					connector_id: PreviewInspectionSessions.connector_id,
					inspection_id: PreviewInspectionSessions.inspection_id,
				})
				.from(PreviewInspectionSessions)
				.where(
					and(
						inArray(PreviewInspectionSessions.state, ["attached", "attaching"]),
						lte(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
					),
				)
				.orderBy(
					asc(PreviewInspectionSessions.requested_at_ms),
					asc(PreviewInspectionSessions.inspection_id),
				)
				.limit(preview_browser_active_inspection_limit)
				.pipe(Effect.mapError(normalize_error));
		const PrepareDetach = (command: Command) =>
			Effect.gen(function* () {
				if (!is_detach_command(command)) {
					return yield* invariant(
						"Inspection detach preparation requires a detach command",
					);
				}

				const encoded = yield* EncodeCommand(command);

				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const replayed = yield* ReplayTransaction(transaction, command, encoded);

						if (replayed._tag === "Completed") {
							return {
								_tag: "Completed",
								acceptance: replayed.acceptance,
							} satisfies PreviewInspectionDetachPreparation;
						}

						if (replayed._tag === "Pending") {
							return yield* invariant(
								"Inspection detach has an unexplained pending command",
							);
						}

						const [row] = yield* transaction
							.select({
								connector_id: PreviewInspectionSessions.connector_id,
								inspection_id: PreviewInspectionSessions.inspection_id,
								state: PreviewInspectionSessions.state,
								thread_id: PreviewInspectionSessions.thread_id,
							})
							.from(PreviewInspectionSessions)
							.where(
								and(
									eq(
										PreviewInspectionSessions.inspection_id,
										command.payload.inspection_id,
									),
									eq(
										PreviewInspectionSessions.project_id,
										command.payload.project_id,
									),
									eq(
										PreviewInspectionSessions.workspace_id,
										command.payload.workspace_id,
									),
								),
							)
							.limit(1);

						if (!row || row.thread_id !== command.thread_id) {
							return yield* new PreviewBrowserRepositoryMissing({
								reason: "inspection",
								subject_id: command.payload.inspection_id,
							});
						}

						if (row.state !== "attached" && row.state !== "attaching") {
							return yield* new PreviewBrowserRepositoryConflict({
								reason: "invalid_transition",
							});
						}

						return {
							_tag: "Prepared",
							revocation: {
								connector_id: row.connector_id,
								inspection_id: row.inspection_id,
							},
						} satisfies PreviewInspectionDetachPreparation;
					}),
				);
			}).pipe(Effect.mapError(normalize_error));

		const DetachInspection = (command: Command, now_ms: number) => {
			if (!is_detach_command(command)) {
				return Effect.fail(invariant("Inspection detach requires a detach command"));
			}

			return Accept(command, (transaction) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(PreviewInspectionSessions)
						.where(
							and(
								eq(
									PreviewInspectionSessions.inspection_id,
									command.payload.inspection_id,
								),
								eq(
									PreviewInspectionSessions.project_id,
									command.payload.project_id,
								),
								eq(
									PreviewInspectionSessions.workspace_id,
									command.payload.workspace_id,
								),
							),
						)
						.limit(1);

					if (!row || row.thread_id !== command.thread_id) {
						return yield* new PreviewBrowserRepositoryMissing({
							reason: "inspection",
							subject_id: command.payload.inspection_id,
						});
					}

					if (row.state !== "attached" && row.state !== "attaching") {
						return yield* new PreviewBrowserRepositoryConflict({
							reason: "invalid_transition",
						});
					}

					const attach_command =
						row.state === "attaching"
							? yield* DecodeCommandEnvelopeJson(row.attach_command_json).pipe(
									Effect.mapError(() =>
										invariant("Stored inspection attach command is corrupt"),
									),
								)
							: undefined;

					if (attach_command !== undefined && !is_attach_command(attach_command)) {
						return yield* invariant(
							"Stored inspection attach command has the wrong type",
						);
					}

					if (attach_command !== undefined) {
						const attach_encoded = yield* EncodeCommand(attach_command);

						yield* ValidateInspectionIntent(row, attach_command, attach_encoded);

						const attach_replay = yield* ReplayTransaction(
							transaction,
							attach_command,
							attach_encoded,
						);

						if (attach_replay._tag !== "Pending") {
							return yield* invariant(
								"Attaching inspection lost its pending command reservation",
							);
						}
					}

					const [updated] = yield* transaction
						.update(PreviewInspectionSessions)
						.set({
							reason: "detached",
							state: "disconnected",
							updated_at_ms: Math.max(now_ms, row.requested_at_ms),
						})
						.where(
							and(
								eq(
									PreviewInspectionSessions.inspection_id,
									command.payload.inspection_id,
								),
								inArray(PreviewInspectionSessions.state, ["attached", "attaching"]),
							),
						)
						.returning();

					if (!updated) {
						return yield* new PreviewBrowserRepositoryConflict({
							reason: "invalid_transition",
						});
					}

					const inspection = yield* DecodeInspection(updated);
					const payload = yield* Schema.decodeUnknownEffect(
						PreviewBrowserLifecycleEvent,
						{
							onExcessProperty: "error",
						},
					)({
						action: "disconnected",
						inspection,
						type: "preview.inspection.updated",
					}).pipe(Effect.mapError(() => invariant("Inspection detach event is invalid")));
					const preceding_events =
						attach_command === undefined
							? []
							: [
									yield* Effect.gen(function* () {
										const [accepted] = yield* transaction
											.update(JournalCommands)
											.set({ status: "accepted" })
											.where(
												and(
													eq(
														JournalCommands.message_id,
														attach_command.message_id,
													),
													eq(JournalCommands.status, "pending"),
												),
											)
											.returning({ message_id: JournalCommands.message_id });

										if (!accepted) {
											return yield* invariant(
												"Inspection attach command settlement was lost",
											);
										}

										return yield* AppendEvent(
											transaction,
											attach_command,
											payload,
											attach_command.message_id,
										);
									}),
								];

					return { payload, preceding_events };
				}),
			);
		};

		const DisconnectInspectionInternal = (
			inspection_id: string,
			reason: "connection_lost" | "interrupted" | "target_changed" | "thread_erased",
			now_ms: number,
			claim?: PreviewBrowserOperationClaim,
			target_removal_claim?: PreviewTargetRemovalClaim,
			require_target_changed = false,
			require_expired = false,
			require_thread_erasure = false,
		) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [row] = yield* transaction
							.select()
							.from(PreviewInspectionSessions)
							.where(eq(PreviewInspectionSessions.inspection_id, inspection_id))
							.limit(1);

						if (!row || (row.state !== "attached" && row.state !== "attaching")) {
							return Option.none<EventEnvelope>();
						}

						if (claim !== undefined) {
							if (
								row.claim_token !== claim.claim_token ||
								row.owner_instance_id !== claim.owner_instance_id ||
								row.lease_expires_at_ms !== claim.lease_expires_at_ms ||
								row.lease_expires_at_ms <= now_ms
							) {
								return Option.none<EventEnvelope>();
							}

							if (require_target_changed) {
								yield* EnsureTargetNotRemoving(transaction, row, now_ms);

								const [target] = yield* transaction
									.select({ generation_id: PreviewTargets.generation_id })
									.from(PreviewTargets)
									.where(
										and(
											eq(PreviewTargets.project_id, row.project_id),
											eq(PreviewTargets.workspace_id, row.workspace_id),
											eq(PreviewTargets.target_id, row.target_id),
										),
									)
									.limit(1);

								if (target?.generation_id === row.target_generation_id) {
									return Option.none<EventEnvelope>();
								}
							} else if (reason !== "target_changed") {
								yield* EnsureOperationTargetCurrent(transaction, row, now_ms);
							}
						}

						if (target_removal_claim !== undefined) {
							yield* EnsureLiveTargetRemovalClaim(
								transaction,
								target_removal_claim,
								now_ms,
							);

							if (
								target_removal_claim.subject._tag === "Missing" ||
								row.project_id !== target_removal_claim.project_id ||
								row.workspace_id !== target_removal_claim.workspace_id ||
								row.target_id !== target_removal_claim.target_id ||
								row.target_generation_id !==
									target_removal_claim.subject.target_generation_id
							) {
								return Option.none<EventEnvelope>();
							}
						}

						if (require_expired && row.lease_expires_at_ms > now_ms) {
							return Option.none<EventEnvelope>();
						}

						if (require_thread_erasure) {
							const [erasure] = yield* transaction
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, row.thread_id))
								.limit(1);

							if (!erasure) {
								return Option.none<EventEnvelope>();
							}
						}

						const command =
							reason === "thread_erased"
								? undefined
								: yield* DecodeCommandEnvelopeJson(row.attach_command_json).pipe(
										Effect.mapError(() =>
											invariant("Stored inspection command is corrupt"),
										),
									);

						if (command !== undefined && !is_attach_command(command)) {
							return yield* invariant("Stored inspection command has the wrong type");
						}

						if (command !== undefined && row.state === "attaching") {
							const encoded = yield* EncodeCommand(command);

							yield* ValidateInspectionIntent(row, command, encoded);

							const replayed = yield* ReplayTransaction(
								transaction,
								command,
								encoded,
							);

							if (replayed._tag !== "Pending") {
								return yield* invariant(
									"Attaching inspection lost its pending command reservation",
								);
							}
						}

						const [updated] = yield* transaction
							.update(PreviewInspectionSessions)
							.set({
								reason,
								state: "disconnected",
								updated_at_ms: Math.max(now_ms, row.requested_at_ms),
							})
							.where(
								and(
									eq(PreviewInspectionSessions.inspection_id, inspection_id),
									inArray(PreviewInspectionSessions.state, [
										"attached",
										"attaching",
									]),
									...(claim === undefined
										? []
										: [
												eq(
													PreviewInspectionSessions.claim_token,
													claim.claim_token,
												),
												eq(
													PreviewInspectionSessions.owner_instance_id,
													claim.owner_instance_id,
												),
												eq(
													PreviewInspectionSessions.lease_expires_at_ms,
													claim.lease_expires_at_ms,
												),
												gt(
													PreviewInspectionSessions.lease_expires_at_ms,
													now_ms,
												),
											]),
									...(require_expired
										? [
												lte(
													PreviewInspectionSessions.lease_expires_at_ms,
													now_ms,
												),
											]
										: []),
								),
							)
							.returning();

						if (!updated) {
							return Option.none<EventEnvelope>();
						}

						if (reason === "thread_erased") {
							return Option.none<EventEnvelope>();
						}

						if (command === undefined) {
							return yield* invariant(
								"Disconnected inspection lost its attach command",
							);
						}

						if (row.state === "attaching") {
							const [accepted] = yield* transaction
								.update(JournalCommands)
								.set({ status: "accepted" })
								.where(
									and(
										eq(JournalCommands.message_id, command.message_id),
										eq(JournalCommands.status, "pending"),
									),
								)
								.returning({ message_id: JournalCommands.message_id });

							if (!accepted) {
								return yield* invariant(
									"Inspection attach command settlement was lost",
								);
							}
						}

						const inspection = yield* DecodeInspection(updated);
						const payload = yield* Schema.decodeUnknownEffect(
							PreviewBrowserLifecycleEvent,
							{ onExcessProperty: "error" },
						)({
							action: "disconnected",
							inspection,
							type: "preview.inspection.updated",
						}).pipe(
							Effect.mapError(() =>
								invariant("Inspection disconnect event is invalid"),
							),
						);
						const event = yield* AppendEvent(
							transaction,
							command,
							payload,
							row.state === "attaching"
								? command.message_id
								: inspection_disconnect_idempotency_key(inspection_id),
						);

						return Option.some(event);
					}),
				),
			).pipe(
				Effect.tap(
					Option.match({
						onNone: () => Effect.void,
						onSome: (event) => notifier.Publish(event.journal_sequence),
					}),
				),
				Effect.mapError(normalize_error),
			);
		const DisconnectExpiredInspection = (
			inspection_id: string,
			reason: "connection_lost" | "interrupted" | "target_changed" | "thread_erased",
			now_ms: number,
		) =>
			DisconnectInspectionInternal(
				inspection_id,
				reason,
				now_ms,
				undefined,
				undefined,
				false,
				true,
			);
		const DisconnectOwnedInspection = (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			reason: "connection_lost" | "interrupted" | "thread_erased",
			now_ms: number,
		) => DisconnectInspectionInternal(inspection_id, reason, now_ms, claim);
		const DisconnectChangedInspection = (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
		) =>
			DisconnectInspectionInternal(
				inspection_id,
				"target_changed",
				now_ms,
				claim,
				undefined,
				true,
			);
		const DisconnectTargetInspection = (
			inspection_id: string,
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) =>
			DisconnectInspectionInternal(inspection_id, "target_changed", now_ms, undefined, claim);
		const DisconnectThreadInspection = (inspection_id: string, now_ms: number) =>
			DisconnectInspectionInternal(
				inspection_id,
				"thread_erased",
				now_ms,
				undefined,
				undefined,
				false,
				false,
				true,
			);

		const RenewInspectionLease = (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
			lease_duration_ms: number,
		) =>
			Effect.gen(function* () {
				const lease_expires_at_ms = yield* lease_expiry(now_ms, lease_duration_ms);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const owned = yield* ReadActiveOwnedInspection(
								transaction,
								inspection_id,
								claim,
								now_ms,
							);
							const renewed_claim = yield* RenewActiveInspectionLease(
								transaction,
								inspection_id,
								claim,
								now_ms,
								lease_expires_at_ms,
							);

							const [erasure] = yield* transaction
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, owned.thread_id))
								.limit(1);

							const target_removing = yield* TargetGenerationRemovalIsPending(
								transaction,
								owned,
							);
							const target_is_current = yield* TargetGenerationIsCurrent(
								transaction,
								owned,
							);
							const cleanup_reason = erasure
								? "thread_erased"
								: target_removing || !target_is_current
									? "target_changed"
									: null;

							return {
								claim: renewed_claim,
								cleanup_reason,
							} satisfies RenewInspectionLeaseResult;
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));
		const RenewInspectionCleanupLease = (
			inspection_id: string,
			claim: PreviewBrowserOperationClaim,
			now_ms: number,
			lease_duration_ms: number,
		) =>
			Effect.gen(function* () {
				const lease_expires_at_ms = yield* lease_expiry(now_ms, lease_duration_ms);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* ReadActiveOwnedInspection(
								transaction,
								inspection_id,
								claim,
								now_ms,
							);

							return yield* RenewActiveInspectionLease(
								transaction,
								inspection_id,
								claim,
								now_ms,
								lease_expires_at_ms,
							);
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));

		const ClaimTargetRemoval = (
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
			now_ms: number,
			lease_duration_ms: number,
		) =>
			Effect.gen(function* () {
				const operation_claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);
				const now = yield* Effect.try({
					try: () => new Date(now_ms).toISOString(),
					catch: () => invariant("Target-removal clock is invalid"),
				});

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* transaction
								.delete(PreviewTargetRemovalClaims)
								.where(
									and(
										eq(PreviewTargetRemovalClaims.project_id, input.project_id),
										eq(
											PreviewTargetRemovalClaims.workspace_id,
											input.workspace_id,
										),
										eq(PreviewTargetRemovalClaims.target_id, input.target_id),
										lte(PreviewTargetRemovalClaims.lease_expires_at_ms, now_ms),
									),
								);
							const [pending_fence] = yield* transaction
								.select({ message_id: PreviewTargetRemovalFences.message_id })
								.from(PreviewTargetRemovalFences)
								.where(
									and(
										eq(PreviewTargetRemovalFences.project_id, input.project_id),
										eq(
											PreviewTargetRemovalFences.workspace_id,
											input.workspace_id,
										),
										eq(PreviewTargetRemovalFences.target_id, input.target_id),
									),
								)
								.limit(1);

							if (pending_fence) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "target_removing",
								});
							}

							const [target] = yield* transaction
								.select({ generation_id: PreviewTargets.generation_id })
								.from(PreviewTargets)
								.where(
									and(
										eq(PreviewTargets.project_id, input.project_id),
										eq(PreviewTargets.workspace_id, input.workspace_id),
										eq(PreviewTargets.target_id, input.target_id),
									),
								)
								.limit(1);
							const subject = target
								? ({
										_tag: "Current" as const,
										target_generation_id: target.generation_id,
									} satisfies PreviewTargetRemovalClaim["subject"])
								: ({
										_tag: "Missing" as const,
									} satisfies PreviewTargetRemovalClaim["subject"]);

							const [inserted] = yield* transaction
								.insert(PreviewTargetRemovalClaims)
								.values({
									...input,
									...operation_claim,
									created_at_ms: now_ms,
									target_generation_id:
										subject._tag === "Current"
											? subject.target_generation_id
											: null,
									updated_at_ms: now_ms,
								})
								.onConflictDoNothing()
								.returning();

							if (!inserted) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "target_removing",
								});
							}

							const [foreign_launch] = yield* transaction
								.select({ message_id: PreviewBrowserLaunches.message_id })
								.from(PreviewBrowserLaunches)
								.where(
									and(
										eq(PreviewBrowserLaunches.project_id, input.project_id),
										eq(PreviewBrowserLaunches.workspace_id, input.workspace_id),
										eq(PreviewBrowserLaunches.target_id, input.target_id),
										inArray(PreviewBrowserLaunches.state, [
											"accepted",
											"dispatching",
										]),
										gt(PreviewBrowserLaunches.lease_expires_at_ms, now_ms),
										ne(
											PreviewBrowserLaunches.owner_instance_id,
											metadata.instance_id,
										),
									),
								)
								.limit(1);
							const [foreign_inspection] = yield* transaction
								.select({ inspection_id: PreviewInspectionSessions.inspection_id })
								.from(PreviewInspectionSessions)
								.where(
									and(
										eq(PreviewInspectionSessions.project_id, input.project_id),
										eq(
											PreviewInspectionSessions.workspace_id,
											input.workspace_id,
										),
										eq(PreviewInspectionSessions.target_id, input.target_id),
										inArray(PreviewInspectionSessions.state, [
											"attached",
											"attaching",
										]),
										gt(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
										ne(
											PreviewInspectionSessions.owner_instance_id,
											metadata.instance_id,
										),
									),
								)
								.limit(1);
							const [active_probe] = yield* transaction
								.select({ message_id: PreviewTargetProbeClaims.message_id })
								.from(PreviewTargetProbeClaims)
								.where(
									and(
										eq(PreviewTargetProbeClaims.project_id, input.project_id),
										eq(
											PreviewTargetProbeClaims.workspace_id,
											input.workspace_id,
										),
										eq(PreviewTargetProbeClaims.target_id, input.target_id),
										gt(PreviewTargetProbeClaims.lease_expires_at, now),
									),
								)
								.limit(1);

							if (foreign_launch || foreign_inspection || active_probe) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "target_removing",
								});
							}

							return {
								...input,
								...operation_claim,
								subject,
							} satisfies PreviewTargetRemovalClaim;
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));

		const ClaimTargetRemovalFence = (
			message_id: string,
			now_ms: number,
			lease_duration_ms: number,
		) =>
			Effect.gen(function* () {
				const operation_claim = yield* MakeOperationClaim(now_ms, lease_duration_ms);

				return yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [stored] = yield* transaction
								.select()
								.from(PreviewTargetRemovalFences)
								.where(eq(PreviewTargetRemovalFences.message_id, message_id))
								.limit(1);

							if (!stored) {
								return Option.none<OwnedPreviewTargetRemovalFence>();
							}

							const fence = yield* DecodeTargetRemovalFence(stored);

							yield* AttestTargetRemovalFence(transaction, fence);

							yield* transaction
								.delete(PreviewTargetRemovalClaims)
								.where(
									and(
										eq(PreviewTargetRemovalClaims.project_id, fence.project_id),
										eq(
											PreviewTargetRemovalClaims.workspace_id,
											fence.workspace_id,
										),
										eq(PreviewTargetRemovalClaims.target_id, fence.target_id),
										lte(PreviewTargetRemovalClaims.lease_expires_at_ms, now_ms),
									),
								);
							const [claimed] = yield* transaction
								.insert(PreviewTargetRemovalClaims)
								.values({
									claim_token: operation_claim.claim_token,
									created_at_ms: now_ms,
									lease_expires_at_ms: operation_claim.lease_expires_at_ms,
									owner_instance_id: operation_claim.owner_instance_id,
									project_id: fence.project_id,
									target_generation_id: fence.target_generation_id,
									target_id: fence.target_id,
									updated_at_ms: now_ms,
									workspace_id: fence.workspace_id,
								})
								.onConflictDoNothing()
								.returning({ claim_token: PreviewTargetRemovalClaims.claim_token });

							if (!claimed) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "target_removing",
								});
							}

							const [foreign_inspection] = yield* transaction
								.select({ inspection_id: PreviewInspectionSessions.inspection_id })
								.from(PreviewInspectionSessions)
								.where(
									and(
										eq(PreviewInspectionSessions.project_id, fence.project_id),
										eq(
											PreviewInspectionSessions.workspace_id,
											fence.workspace_id,
										),
										eq(PreviewInspectionSessions.target_id, fence.target_id),
										eq(
											PreviewInspectionSessions.target_generation_id,
											fence.target_generation_id,
										),
										inArray(PreviewInspectionSessions.state, [
											"attached",
											"attaching",
										]),
										gt(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
										ne(
											PreviewInspectionSessions.owner_instance_id,
											metadata.instance_id,
										),
									),
								)
								.limit(1);

							if (foreign_inspection) {
								return yield* new PreviewBrowserRepositoryUnavailable({
									reason: "target_removing",
								});
							}

							return Option.some({
								claim: {
									...operation_claim,
									project_id: fence.project_id,
									subject: {
										_tag: "Current",
										target_generation_id: fence.target_generation_id,
									},
									target_id: fence.target_id,
									workspace_id: fence.workspace_id,
								},
								fence,
							} satisfies OwnedPreviewTargetRemovalFence);
						}),
					),
				);
			}).pipe(Effect.mapError(normalize_error));

		const CompleteTargetRemovalFence = (
			owned: OwnedPreviewTargetRemovalFence,
			now_ms: number,
		) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [stored] = yield* transaction
							.select()
							.from(PreviewTargetRemovalFences)
							.where(
								eq(PreviewTargetRemovalFences.message_id, owned.fence.message_id),
							)
							.limit(1);

						if (!stored) {
							return;
						}

						const fence = yield* DecodeTargetRemovalFence(stored);

						if (
							fence.committed_at_ms !== owned.fence.committed_at_ms ||
							fence.project_id !== owned.fence.project_id ||
							fence.target_generation_id !== owned.fence.target_generation_id ||
							fence.target_id !== owned.fence.target_id ||
							fence.thread_id !== owned.fence.thread_id ||
							fence.workspace_id !== owned.fence.workspace_id ||
							fence.project_id !== owned.claim.project_id ||
							fence.target_id !== owned.claim.target_id ||
							fence.workspace_id !== owned.claim.workspace_id ||
							owned.claim.subject._tag !== "Current" ||
							owned.claim.subject.target_generation_id !== fence.target_generation_id
						) {
							return yield* invariant("Target-removal fence ownership is corrupt");
						}

						yield* EnsureLiveTargetRemovalClaim(transaction, owned.claim, now_ms);

						const [inspection] = yield* transaction
							.select({ inspection_id: PreviewInspectionSessions.inspection_id })
							.from(PreviewInspectionSessions)
							.where(
								and(
									eq(PreviewInspectionSessions.project_id, fence.project_id),
									eq(PreviewInspectionSessions.workspace_id, fence.workspace_id),
									eq(PreviewInspectionSessions.target_id, fence.target_id),
									eq(
										PreviewInspectionSessions.target_generation_id,
										fence.target_generation_id,
									),
									inArray(PreviewInspectionSessions.state, [
										"attached",
										"attaching",
									]),
								),
							)
							.limit(1);

						if (inspection) {
							return yield* new PreviewBrowserRepositoryUnavailable({
								reason: "target_removing",
							});
						}

						const [completed] = yield* transaction
							.delete(PreviewTargetRemovalFences)
							.where(
								and(
									eq(PreviewTargetRemovalFences.message_id, fence.message_id),
									eq(PreviewTargetRemovalFences.project_id, fence.project_id),
									eq(PreviewTargetRemovalFences.workspace_id, fence.workspace_id),
									eq(PreviewTargetRemovalFences.target_id, fence.target_id),
									eq(
										PreviewTargetRemovalFences.target_generation_id,
										fence.target_generation_id,
									),
								),
							)
							.returning({ message_id: PreviewTargetRemovalFences.message_id });

						if (completed) {
							return;
						}

						const [current] = yield* transaction
							.select({ message_id: PreviewTargetRemovalFences.message_id })
							.from(PreviewTargetRemovalFences)
							.where(eq(PreviewTargetRemovalFences.message_id, fence.message_id))
							.limit(1);

						if (!current) {
							return;
						}

						return yield* new PreviewBrowserRepositoryUnavailable({
							reason: "ownership_lost",
						});
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const RenewTargetRemoval = (
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
			lease_duration_ms: number,
		) =>
			Effect.gen(function* () {
				const lease_expires_at_ms = yield* lease_expiry(now_ms, lease_duration_ms);
				const [renewed] = yield* RetrySqliteWrite(
					database.client
						.update(PreviewTargetRemovalClaims)
						.set({ lease_expires_at_ms, updated_at_ms: now_ms })
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
								eq(
									PreviewTargetRemovalClaims.lease_expires_at_ms,
									claim.lease_expires_at_ms,
								),
								gt(PreviewTargetRemovalClaims.lease_expires_at_ms, now_ms),
							),
						)
						.returning({
							lease_expires_at_ms: PreviewTargetRemovalClaims.lease_expires_at_ms,
						}),
				);

				if (!renewed) {
					return yield* new PreviewBrowserRepositoryUnavailable({
						reason: "ownership_lost",
					});
				}

				return { ...claim, lease_expires_at_ms: renewed.lease_expires_at_ms };
			}).pipe(Effect.mapError(normalize_error));

		const ReleaseTargetRemoval = (claim: PreviewTargetRemovalClaim) =>
			RetrySqliteWrite(
				database.client
					.delete(PreviewTargetRemovalClaims)
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
						),
					),
			).pipe(Effect.asVoid, Effect.mapError(normalize_error));

		const ActiveInspectionIdsForTargetRemoval = (
			claim: PreviewTargetRemovalClaim,
			now_ms: number,
		) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* EnsureLiveTargetRemovalClaim(transaction, claim, now_ms);

						if (claim.subject._tag === "Missing") {
							return [];
						}

						const rows = yield* transaction
							.select({ inspection_id: PreviewInspectionSessions.inspection_id })
							.from(PreviewInspectionSessions)
							.where(
								and(
									eq(PreviewInspectionSessions.project_id, claim.project_id),
									eq(PreviewInspectionSessions.workspace_id, claim.workspace_id),
									eq(PreviewInspectionSessions.target_id, claim.target_id),
									eq(
										PreviewInspectionSessions.target_generation_id,
										claim.subject.target_generation_id,
									),
									inArray(PreviewInspectionSessions.state, [
										"attached",
										"attaching",
									]),
								),
							)
							.orderBy(asc(PreviewInspectionSessions.inspection_id))
							.limit(preview_browser_active_inspection_limit);

						return rows.map((row) => row.inspection_id);
					}),
				),
			).pipe(Effect.mapError(normalize_error));

		const ActiveInspectionIdsForThread = (thread_id: string) =>
			database.client
				.select({ inspection_id: PreviewInspectionSessions.inspection_id })
				.from(PreviewInspectionSessions)
				.where(
					and(
						eq(PreviewInspectionSessions.thread_id, thread_id),
						inArray(PreviewInspectionSessions.state, ["attached", "attaching"]),
					),
				)
				.orderBy(asc(PreviewInspectionSessions.inspection_id))
				.limit(preview_browser_active_inspection_limit)
				.pipe(
					Effect.map((rows) => rows.map((row) => row.inspection_id)),
					Effect.mapError(normalize_error),
				);

		const List = (input: { readonly project_id: string; readonly workspace_id: string }) =>
			Effect.gen(function* () {
				const launch_rows = yield* database.client
					.select()
					.from(PreviewBrowserLaunches)
					.where(
						and(
							eq(PreviewBrowserLaunches.project_id, input.project_id),
							eq(PreviewBrowserLaunches.workspace_id, input.workspace_id),
						),
					)
					.orderBy(
						desc(PreviewBrowserLaunches.updated_at_ms),
						asc(PreviewBrowserLaunches.message_id),
					)
					.limit(256);
				const inspection_rows = yield* database.client
					.select()
					.from(PreviewInspectionSessions)
					.where(
						and(
							eq(PreviewInspectionSessions.project_id, input.project_id),
							eq(PreviewInspectionSessions.workspace_id, input.workspace_id),
						),
					)
					.orderBy(
						desc(PreviewInspectionSessions.updated_at_ms),
						asc(PreviewInspectionSessions.inspection_id),
					)
					.limit(256);
				const launches = yield* Effect.forEach(launch_rows, DecodeLaunch);
				const inspections = yield* Effect.forEach(inspection_rows, DecodeInspection);

				return yield* Schema.decodeUnknownEffect(PreviewBrowserLifecycleQueryResult, {
					onExcessProperty: "error",
				})({ ...input, inspections, launches }).pipe(
					Effect.mapError(() => invariant("Browser lifecycle query result is invalid")),
				);
			}).pipe(Effect.mapError(normalize_error));

		const ListTargetRemovalFences = (input?: { readonly thread_id?: string }) =>
			(input?.thread_id === undefined
				? database.client
						.select()
						.from(PreviewTargetRemovalFences)
						.orderBy(
							asc(PreviewTargetRemovalFences.committed_at_ms),
							asc(PreviewTargetRemovalFences.message_id),
						)
						.limit(preview_browser_active_inspection_limit)
				: database.client
						.select()
						.from(PreviewTargetRemovalFences)
						.where(eq(PreviewTargetRemovalFences.thread_id, input.thread_id))
						.orderBy(
							asc(PreviewTargetRemovalFences.committed_at_ms),
							asc(PreviewTargetRemovalFences.message_id),
						)
						.limit(preview_browser_active_inspection_limit)
			).pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeTargetRemovalFence)),
				Effect.mapError(normalize_error),
			);

		const RecoverInterrupted = (
			now_ms: number,
			revoked_inspection_ids: ReadonlyArray<string>,
		) =>
			Effect.gen(function* () {
				const revoked_ids = [...new Set(revoked_inspection_ids)];

				if (
					revoked_ids.length > preview_browser_active_inspection_limit ||
					revoked_ids.some((inspection_id) => inspection_id.length === 0)
				) {
					return yield* invariant("Revoked inspection recovery batch is invalid");
				}

				const launch_rows = yield* database.client
					.select()
					.from(PreviewBrowserLaunches)
					.where(
						and(
							inArray(PreviewBrowserLaunches.state, ["accepted", "dispatching"]),
							lte(PreviewBrowserLaunches.lease_expires_at_ms, now_ms),
						),
					)
					.orderBy(
						asc(PreviewBrowserLaunches.requested_at_ms),
						asc(PreviewBrowserLaunches.message_id),
					)
					.limit(preview_browser_active_inspection_limit);
				const inspection_limit =
					preview_browser_active_inspection_limit - launch_rows.length;
				const inspection_rows =
					inspection_limit === 0 || revoked_ids.length === 0
						? []
						: yield* database.client
								.select()
								.from(PreviewInspectionSessions)
								.where(
									and(
										inArray(PreviewInspectionSessions.state, [
											"attached",
											"attaching",
										]),
										inArray(
											PreviewInspectionSessions.inspection_id,
											revoked_ids,
										),
										lte(PreviewInspectionSessions.lease_expires_at_ms, now_ms),
									),
								)
								.orderBy(
									asc(PreviewInspectionSessions.requested_at_ms),
									asc(PreviewInspectionSessions.inspection_id),
								)
								.limit(inspection_limit);
				const inspection_thread_ids = [
					...new Set(inspection_rows.map((row) => row.thread_id)),
				];
				const erasing_threads =
					inspection_thread_ids.length === 0
						? []
						: yield* database.client
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(
									inArray(ThreadErasureClaims.thread_id, inspection_thread_ids),
								);
				const erasing_thread_ids = new Set(erasing_threads.map((row) => row.thread_id));
				const target_removing_inspection_ids = new Set(
					(yield* Effect.forEach(
						inspection_rows,
						(row) =>
							TargetGenerationRemovalIsPending(database.client, row).pipe(
								Effect.map((pending) =>
									pending
										? Option.some(row.inspection_id)
										: Option.none<string>(),
								),
							),
						{ concurrency: 1 },
					)).flatMap(Option.toArray),
				);
				const launch_events = yield* Effect.forEach(
					launch_rows,
					(row) =>
						DecodeCommandEnvelopeJson(row.command_json).pipe(
							Effect.mapError(() =>
								invariant("Stored browser launch command is corrupt"),
							),
							Effect.flatMap((command) =>
								SettleLaunchInternal(
									command,
									operation_claim(row),
									{ reason: "interrupted", state: "outcome_unknown" },
									now_ms,
									"expired",
								),
							),
							Effect.map((acceptance) =>
								acceptance.status === "accepted"
									? Option.some(acceptance.event)
									: Option.none<EventEnvelope>(),
							),
						),
					{ concurrency: 1 },
				);
				const inspection_events = yield* Effect.forEach(
					inspection_rows,
					(row) => {
						const reason = erasing_thread_ids.has(row.thread_id)
							? "thread_erased"
							: target_removing_inspection_ids.has(row.inspection_id)
								? "target_changed"
								: "interrupted";

						return row.state === "attaching"
							? DecodeCommandEnvelopeJson(row.attach_command_json).pipe(
									Effect.mapError(() =>
										invariant("Stored inspection attach command is corrupt"),
									),
									Effect.flatMap((command) =>
										SettleInspectionAttachInternal(
											command,
											operation_claim(row),
											{ reason, state: "disconnected" },
											now_ms,
											"expired",
										),
									),
									Effect.map((acceptance) =>
										acceptance.status === "accepted"
											? Option.some(acceptance.event)
											: Option.none<EventEnvelope>(),
									),
								)
							: DisconnectExpiredInspection(row.inspection_id, reason, now_ms);
					},
					{ concurrency: 1 },
				);

				return [...launch_events, ...inspection_events].flatMap(Option.toArray);
			}).pipe(Effect.mapError(normalize_error));

		return {
			ActiveInspectionIdsForTargetRemoval,
			ActiveInspectionIdsForThread,
			ClaimTargetRemoval,
			ClaimTargetRemovalFence,
			CompleteTargetRemovalFence,
			DetachInspection,
			DisconnectChangedInspection,
			DisconnectOwnedInspection,
			DisconnectTargetInspection,
			DisconnectThreadInspection,
			InspectionRevocation,
			List,
			ListExpiredInspectionRevocations,
			ListTargetRemovalFences,
			PrepareDetach,
			PrepareInspection,
			PrepareLaunch,
			RecoverInterrupted,
			ReleaseTargetRemoval,
			Replay,
			RenewInspectionCleanupLease,
			RenewInspectionLease,
			RenewTargetRemoval,
			SettleInspectionAttach,
			SettleLaunch,
		};
	}),
);

/** Maps repository failures into the lifecycle service's source-safe error vocabulary. */
export function map_preview_browser_repository_error(
	error: PreviewBrowserRepositoryError,
	subject_id: string,
): PreviewBrowserLifecycleError {
	if (error instanceof PreviewBrowserRepositoryConflict) {
		return new PreviewBrowserLifecycleError({ code: "conflict", subject_id });
	}

	if (error instanceof PreviewBrowserRepositoryMissing) {
		return new PreviewBrowserLifecycleError({ code: "not_found", subject_id });
	}

	if (error instanceof PreviewBrowserRepositoryInvariant) {
		return new PreviewBrowserLifecycleError({ code: "invariant", subject_id });
	}

	return new PreviewBrowserLifecycleError({ code: "unavailable", subject_id });
}
