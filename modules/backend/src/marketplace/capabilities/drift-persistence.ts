import { eq } from "drizzle-orm";
import { Context, Effect, Schema } from "effect";

import {
	CapabilityDetail,
	MarketplaceScope,
	ProviderSyncState,
	type MarketplaceLedgerEvent,
} from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import {
	MarketplaceCapabilityMirrors,
	MarketplaceCapabilityOperations,
} from "../../persistence/tables";
import type { CapabilityRepositoryError } from "./repository";

type DriftTransaction = Parameters<Parameters<DatabaseClient["transaction"]>[0]>[0];

interface DriftAppendInput {
	readonly capability_id: string;
	readonly operation: MarketplaceLedgerEvent["operation"];
	readonly operation_id: string;
	readonly status: CapabilityDetail["status"];
}

const DriftIntent = Schema.Struct({
	action: Schema.Union([
		Schema.Literal("ignore"),
		Schema.Literal("import"),
		Schema.Literal("overwrite"),
	]),
	engine_id: Schema.String,
	observed_revision: Schema.String,
	scope: Schema.optional(MarketplaceScope),
});

const OverwriteIntent = Schema.Struct({
	action: Schema.Literal("overwrite"),
	engine_id: Schema.String,
	observed_revision: Schema.String,
	scope: MarketplaceScope,
});

const SyncIntent = Schema.Struct({
	engine_id: Schema.String,
});

export interface DriftPersistence {
	readonly RecordDriftResolution: (input: {
		readonly approval_fingerprint?: string;
		readonly approval_id?: string;
		readonly action: "ignore" | "import" | "overwrite";
		readonly capability_id: string;
		readonly engine_id: string;
		readonly observed_revision: string;
		readonly operation_id: string;
		readonly scope?: MarketplaceScope;
	}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
	readonly DecideDriftOverwrite: (input: {
		readonly approval_fingerprint: string;
		readonly approval_id: string;
		readonly approved: boolean;
	}) => Effect.Effect<
		"approved" | "completed" | "denied" | "duplicate",
		CapabilityRepositoryError
	>;
	readonly ReadDriftApproval: (approval_id: string) => Effect.Effect<
		{
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly scope: MarketplaceScope;
		},
		CapabilityRepositoryError
	>;
	readonly ClaimDriftOverwrite: (
		operation_id: string,
	) => Effect.Effect<"claimed" | "completed" | "writing", CapabilityRepositoryError>;
	readonly CompleteDriftResolution: (input: {
		readonly capability_id: string;
		readonly operation_id: string;
		readonly state: ProviderSyncState;
		readonly status: CapabilityDetail["status"];
	}) => Effect.Effect<void, CapabilityRepositoryError>;
	readonly RecordProviderSync: (input: {
		readonly capability_id: string;
		readonly engine_id: string;
		readonly operation_id: string;
	}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
	readonly ClaimProviderSync: (
		operation_id: string,
	) => Effect.Effect<"claimed" | "completed" | "syncing", CapabilityRepositoryError>;
	readonly CompleteProviderSync: (input: {
		readonly capability_id: string;
		readonly operation_id: string;
		readonly state: ProviderSyncState;
		readonly status: CapabilityDetail["status"];
	}) => Effect.Effect<void, CapabilityRepositoryError>;
}

export class DriftPersistenceContext extends Context.Service<
	DriftPersistenceContext,
	{
		readonly Append: (
			transaction: DriftTransaction,
			input: DriftAppendInput,
		) => Effect.Effect<number, unknown>;
		readonly database: { readonly client: DatabaseClient };
		readonly IsError: (error: unknown) => error is CapabilityRepositoryError;
		readonly MakeError: (
			code: CapabilityRepositoryError["code"],
			message: string,
		) => CapabilityRepositoryError;
		readonly metadata: { readonly Now: Effect.Effect<string> };
		readonly notifier: { readonly Publish: (sequence: number) => Effect.Effect<void, unknown> };
	}
>()("Artisan/Marketplace/DriftPersistenceContext") {}

export const MakeDriftPersistence = Effect.gen(function* () {
	const { Append, database, IsError, MakeError, metadata, notifier } =
		yield* DriftPersistenceContext;
	const RepositoryError = (message: string) => (error: unknown) =>
		IsError(error) ? error : MakeError("invariant", message);
	const EncodeDriftIntent = (value: unknown) =>
		Schema.decodeUnknownEffect(DriftIntent)(value).pipe(
			Effect.map(JSON.stringify),
			Effect.mapError(() => MakeError("invariant", "Capability drift intent is invalid")),
		);
	const EncodeSyncIntent = (value: unknown) =>
		Schema.decodeUnknownEffect(SyncIntent)(value).pipe(
			Effect.map(JSON.stringify),
			Effect.mapError(() => MakeError("invariant", "Provider sync intent is invalid")),
		);
	const DecodeJson = <A>(
		schema: Schema.Codec<A, unknown, never, never>,
		value: string,
		message: string,
	) =>
		Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(schema)),
			Effect.mapError(() => MakeError("invariant", message)),
		);

	const RecordDriftResolution: DriftPersistence["RecordDriftResolution"] = (input) =>
		Effect.gen(function* () {
			const request_fingerprint = yield* EncodeDriftIntent({
				action: input.action,
				engine_id: input.engine_id,
				observed_revision: input.observed_revision,
				...(input.scope === undefined ? {} : { scope: input.scope }),
			});
			return yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [existing] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (existing) {
						if (
							existing.kind !== "drift" ||
							existing.capability_id !== input.capability_id ||
							existing.request_fingerprint !== request_fingerprint ||
							existing.approval_id !== (input.approval_id ?? null) ||
							existing.approval_fingerprint !== (input.approval_fingerprint ?? null)
						)
							return yield* MakeError(
								"conflict",
								"Capability drift id was reused with different intent",
							);
						return "duplicate";
					}
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						approval_id: input.approval_id ?? null,
						approval_fingerprint: input.approval_fingerprint ?? null,
						capability_id: input.capability_id,
						created_at: now,
						kind: "drift",
						operation_id: input.operation_id,
						request_fingerprint,
						state: input.action === "overwrite" ? "awaiting_approval" : "claimed",
						updated_at: now,
					});
					return "accepted";
				}),
			);
		}).pipe(Effect.mapError(RepositoryError("Capability drift action could not be persisted")));

	const DecideDriftOverwrite: DriftPersistence["DecideDriftOverwrite"] = (input) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.approval_id, input.approval_id))
						.limit(1);
					if (
						!operation ||
						operation.kind !== "drift" ||
						operation.approval_fingerprint !== input.approval_fingerprint
					)
						return yield* MakeError(
							"conflict",
							"Drift approval does not match its reviewed request",
						);
					if (operation.state === "completed" && input.approved) return "completed";
					const state: "approved" | "denied" = input.approved ? "approved" : "denied";
					if (operation.state === state) return "duplicate";
					if (operation.state !== "awaiting_approval")
						return yield* MakeError("conflict", "Drift approval was already decided");
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ approval_decision: state, state, updated_at: now })
						.where(
							eq(
								MarketplaceCapabilityOperations.operation_id,
								operation.operation_id,
							),
						);
					return state;
				}),
			)
			.pipe(Effect.mapError(RepositoryError("Drift approval could not be persisted")));

	const ReadDriftApproval: DriftPersistence["ReadDriftApproval"] = (approval_id) =>
		database.client
			.select()
			.from(MarketplaceCapabilityOperations)
			.where(eq(MarketplaceCapabilityOperations.approval_id, approval_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([operation]) => {
					if (!operation || operation.kind !== "drift")
						return Effect.fail(MakeError("not_found", "Drift approval was not found"));
					return DecodeJson(
						OverwriteIntent,
						operation.request_fingerprint,
						"Drift approval intent is corrupt",
					).pipe(
						Effect.map((intent) => ({
							capability_id: operation.capability_id,
							engine_id: intent.engine_id,
							observed_revision: intent.observed_revision,
							operation_id: operation.operation_id,
							scope: intent.scope,
						})),
					);
				}),
				Effect.mapError(RepositoryError("Drift approval could not be read")),
			);

	const ClaimDriftOverwrite: DriftPersistence["ClaimDriftOverwrite"] = (operation_id) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "drift")
						return yield* MakeError(
							"conflict",
							"Capability drift claim has no matching request",
						);
					if (operation.state === "completed") return "completed";
					if (operation.state === "writing") return "writing";
					if (operation.state !== "approved")
						return yield* MakeError(
							"conflict",
							"Capability drift overwrite is not approved",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "writing", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return "claimed";
				}),
			)
			.pipe(
				Effect.mapError(RepositoryError("Capability drift claim could not be persisted")),
			);

	const CompleteMirror = (
		transaction: DriftTransaction,
		capability_id: string,
		state: ProviderSyncState,
	) =>
		transaction
			.insert(MarketplaceCapabilityMirrors)
			.values({
				capability_id,
				engine_id: state.engine_id,
				status: state.status,
				observed_revision: state.observed_revision ?? null,
				last_error_code: state.last_error_code ?? null,
				updated_at: state.updated_at,
			})
			.onConflictDoUpdate({
				target: [
					MarketplaceCapabilityMirrors.capability_id,
					MarketplaceCapabilityMirrors.engine_id,
				],
				set: {
					status: state.status,
					observed_revision: state.observed_revision ?? null,
					last_error_code: state.last_error_code ?? null,
					updated_at: state.updated_at,
				},
			});

	const CompleteDriftResolution: DriftPersistence["CompleteDriftResolution"] = (input) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const now = yield* metadata.Now;
					yield* CompleteMirror(transaction, input.capability_id, input.state);
					const sequence = yield* Append(transaction, {
						capability_id: input.capability_id,
						operation: "drift_resolved",
						operation_id: input.operation_id,
						status: input.status,
					});
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "completed", updated_at: now })
						.where(
							eq(MarketplaceCapabilityOperations.operation_id, input.operation_id),
						);
					return sequence;
				}),
			)
			.pipe(
				Effect.tap((sequence) => notifier.Publish(sequence)),
				Effect.asVoid,
				Effect.mapError(
					RepositoryError("Capability drift completion could not be persisted"),
				),
			);

	const RecordProviderSync: DriftPersistence["RecordProviderSync"] = (input) =>
		Effect.gen(function* () {
			const request_fingerprint = yield* EncodeSyncIntent({ engine_id: input.engine_id });
			return yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [existing] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (existing) {
						if (
							existing.kind !== "sync" ||
							existing.capability_id !== input.capability_id ||
							existing.request_fingerprint !== request_fingerprint
						)
							return yield* MakeError(
								"conflict",
								"Provider sync id was reused with different intent",
							);
						return "duplicate";
					}
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						capability_id: input.capability_id,
						created_at: now,
						kind: "sync",
						operation_id: input.operation_id,
						request_fingerprint,
						state: "pending",
						updated_at: now,
					});
					return "accepted";
				}),
			);
		}).pipe(Effect.mapError(RepositoryError("Provider sync request could not be persisted")));

	const ClaimProviderSync: DriftPersistence["ClaimProviderSync"] = (operation_id) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "sync")
						return yield* MakeError("conflict", "Provider sync claim has no request");
					if (operation.state === "completed") return "completed";
					if (operation.state === "syncing") return "syncing";
					if (operation.state !== "pending")
						return yield* MakeError("conflict", "Provider sync is not claimable");
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "syncing", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return "claimed";
				}),
			)
			.pipe(Effect.mapError(RepositoryError("Provider sync claim could not be persisted")));

	const CompleteProviderSync: DriftPersistence["CompleteProviderSync"] = (input) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (
						!operation ||
						operation.kind !== "sync" ||
						operation.capability_id !== input.capability_id ||
						operation.state !== "syncing"
					)
						return yield* MakeError(
							"conflict",
							"Provider sync completion has no matching claim",
						);
					yield* CompleteMirror(transaction, input.capability_id, input.state);
					const sequence = yield* Append(transaction, {
						capability_id: input.capability_id,
						operation: "synced",
						operation_id: input.operation_id,
						status: input.status,
					});
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "completed", updated_at: now })
						.where(
							eq(MarketplaceCapabilityOperations.operation_id, input.operation_id),
						);
					return sequence;
				}),
			)
			.pipe(
				Effect.tap((sequence) => notifier.Publish(sequence)),
				Effect.asVoid,
				Effect.mapError(RepositoryError("Provider sync completion could not be persisted")),
			);

	return {
		ClaimDriftOverwrite,
		ClaimProviderSync,
		CompleteDriftResolution,
		CompleteProviderSync,
		DecideDriftOverwrite,
		ReadDriftApproval,
		RecordDriftResolution,
		RecordProviderSync,
	};
});
