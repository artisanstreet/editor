import { eq } from "drizzle-orm";
import { Context, Effect } from "effect";

import type { CapabilityDetail, CapabilityHealth, MarketplaceLedgerEvent } from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import { MarketplaceCapabilities, MarketplaceCapabilityOperations } from "../../persistence/tables";
import type { CapabilityRepositoryError } from "./repository";

type LifecycleTransaction = Parameters<Parameters<DatabaseClient["transaction"]>[0]>[0];

interface LifecycleAppendInput {
	readonly capability_id: string;
	readonly health?: CapabilityHealth;
	readonly operation: MarketplaceLedgerEvent["operation"];
	readonly operation_id: string;
	readonly status: CapabilityDetail["status"];
}

export interface LifecyclePersistence {
	readonly ClaimConnect: (
		operation_id: string,
	) => Effect.Effect<"claimed" | "connected" | "denied" | "connecting", unknown>;
	readonly ClaimSessionAction: (
		operation_id: string,
	) => Effect.Effect<"claimed" | "completed" | "executing", CapabilityRepositoryError>;
	readonly ClaimUninstall: (
		operation_id: string,
	) => Effect.Effect<"claimed" | "closing" | "uninstalled", CapabilityRepositoryError>;
	readonly CompleteSessionAction: (input: {
		readonly action: "start" | "reconnect" | "restart";
		readonly detail: CapabilityDetail;
		readonly operation_id: string;
		readonly server_metadata?: Readonly<Record<string, string>>;
	}) => Effect.Effect<void, CapabilityRepositoryError>;
	readonly CompleteUninstall: (
		operation_id: string,
	) => Effect.Effect<void, CapabilityRepositoryError>;
	readonly DecideConnect: (input: {
		readonly approval_fingerprint: string;
		readonly approval_id: string;
		readonly approved: boolean;
	}) => Effect.Effect<
		"approved" | "connected" | "denied" | "duplicate",
		CapabilityRepositoryError
	>;
	readonly ReadApprovedConnect: (
		capability_id: string,
	) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
	readonly ReadConnectApproval: (approval_id: string) => Effect.Effect<
		{
			readonly approval_fingerprint: string;
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
		},
		CapabilityRepositoryError
	>;
	readonly ReadConnectRequest: (
		operation_id: string,
	) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
	readonly RecordConnectRequest: (input: {
		readonly approval_fingerprint?: string;
		readonly approval_id?: string;
		readonly capability_id: string;
		readonly detail: CapabilityDetail;
		readonly operation_id: string;
		readonly request_fingerprint: string;
	}) => Effect.Effect<"accepted" | "duplicate", unknown>;
	readonly RecordSessionAction: (input: {
		readonly action: "start" | "reconnect" | "restart";
		readonly capability_id: string;
		readonly operation_id: string;
		readonly request_fingerprint: string;
	}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
	readonly RecordUninstall: (input: {
		readonly capability_id: string;
		readonly operation_id: string;
	}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
}

export class LifecyclePersistenceContext extends Context.Service<
	LifecyclePersistenceContext,
	{
		readonly Append: (
			transaction: LifecycleTransaction,
			input: LifecycleAppendInput,
		) => Effect.Effect<number, unknown>;
		readonly DecodeDetail: (
			value: string,
			field: string,
		) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
		readonly database: { readonly client: DatabaseClient };
		readonly IsError: (error: unknown) => error is CapabilityRepositoryError;
		readonly MakeError: (
			code: CapabilityRepositoryError["code"],
			message: string,
		) => CapabilityRepositoryError;
		readonly metadata: {
			readonly Now: Effect.Effect<string>;
		};
		readonly notifier: {
			readonly Publish: (journal_sequence: number) => Effect.Effect<void>;
		};
	}
>()("Artisan/Marketplace/LifecyclePersistenceContext") {}

export const MakeLifecyclePersistence = Effect.gen(function* () {
	const { Append, database, DecodeDetail, IsError, MakeError, metadata, notifier } =
		yield* LifecyclePersistenceContext;

	const RepositoryError = (message: string) => (error: unknown) =>
		IsError(error) ? error : MakeError("invariant", message);

	const RecordConnectRequest = (input: {
		readonly approval_fingerprint?: string;
		readonly approval_id?: string;
		readonly capability_id: string;
		readonly detail: CapabilityDetail;
		readonly operation_id: string;
		readonly request_fingerprint: string;
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [existing] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					const detail_json = JSON.stringify(input.detail);
					if (existing) {
						if (
							existing.capability_id !== input.capability_id ||
							existing.approval_id !== (input.approval_id ?? null) ||
							existing.approval_fingerprint !==
								(input.approval_fingerprint ?? null) ||
							existing.detail_json !== detail_json ||
							existing.request_fingerprint !== input.request_fingerprint ||
							existing.kind !== "connect"
						)
							return yield* MakeError(
								"conflict",
								"Capability operation id was reused with different intent",
							);
						return "duplicate" as const;
					}
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						approval_fingerprint: input.approval_fingerprint ?? null,
						approval_id: input.approval_id ?? null,
						capability_id: input.capability_id,
						created_at: now,
						detail_json,
						kind: "connect",
						operation_id: input.operation_id,
						request_fingerprint: input.request_fingerprint,
						state: "awaiting_approval",
						updated_at: now,
					});
					return "accepted" as const;
				}),
			)
			.pipe(
				Effect.mapError(
					RepositoryError("Capability connection request could not be persisted"),
				),
			);

	const DecideConnect = (input: {
		readonly approval_fingerprint: string;
		readonly approval_id: string;
		readonly approved: boolean;
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.approval_id, input.approval_id))
						.limit(1);
					if (!operation || operation.kind !== "connect")
						return yield* MakeError(
							"conflict",
							"Capability approval has no matching request",
						);
					if (operation.approval_fingerprint !== input.approval_fingerprint)
						return yield* MakeError(
							"conflict",
							"Capability approval fingerprint does not match reviewed detail",
						);
					const state: "approved" | "denied" = input.approved ? "approved" : "denied";
					if (operation.state === "connected" && input.approved)
						return "connected" as const;
					if (operation.state === state) return "duplicate" as const;
					if (operation.state !== "awaiting_approval")
						return yield* MakeError(
							"conflict",
							"Capability approval was already decided",
						);
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
			.pipe(Effect.mapError(RepositoryError("Capability approval could not be persisted")));

	const ReadConnectRequest = (operation_id: string) =>
		database.client
			.select()
			.from(MarketplaceCapabilityOperations)
			.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([operation]) =>
					!operation || operation.kind !== "connect" || operation.detail_json === null
						? Effect.fail(
								MakeError(
									"invariant",
									"Capability connection has no reviewed detail",
								),
							)
						: DecodeDetail(operation.detail_json, "reviewed connection detail"),
				),
				Effect.mapError(RepositoryError("Capability connection request could not be read")),
			);

	const ReadConnectApproval = (approval_id: string) =>
		database.client
			.select()
			.from(MarketplaceCapabilityOperations)
			.where(eq(MarketplaceCapabilityOperations.approval_id, approval_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([operation]) => {
					if (
						!operation ||
						operation.kind !== "connect" ||
						operation.approval_fingerprint === null ||
						operation.detail_json === null
					)
						return Effect.fail(
							MakeError("not_found", "Capability approval request was not found"),
						);
					const approval_fingerprint = operation.approval_fingerprint;
					return DecodeDetail(operation.detail_json, "reviewed connection detail").pipe(
						Effect.map((detail) => ({
							approval_fingerprint,
							detail,
							operation_id: operation.operation_id,
						})),
					);
				}),
				Effect.mapError(RepositoryError("Capability approval request could not be read")),
			);

	const ClaimConnect = (operation_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "connect")
						return yield* MakeError(
							"conflict",
							"Capability connection claim has no matching request",
						);
					if (operation.state === "connected") return "connected" as const;
					if (operation.state === "denied") return "denied" as const;
					if (operation.state === "connecting") return "connecting" as const;
					if (operation.state !== "approved")
						return yield* MakeError(
							"conflict",
							"Capability connection is not approved",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "connecting", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return "claimed" as const;
				}),
			)
			.pipe(
				Effect.mapError(
					RepositoryError("Capability connection claim could not be persisted"),
				),
			);

	const ReadApprovedConnect = (capability_id: string) =>
		database.client
			.select()
			.from(MarketplaceCapabilityOperations)
			.where(eq(MarketplaceCapabilityOperations.capability_id, capability_id))
			.pipe(
				Effect.flatMap((operations) => {
					const approved = [...operations]
						.reverse()
						.find(
							(operation) =>
								operation.kind === "connect" &&
								operation.approval_decision === "approved" &&
								operation.detail_json !== null,
						);
					return approved?.detail_json
						? DecodeDetail(approved.detail_json, "approved connect detail")
						: Effect.fail(
								MakeError(
									"not_found",
									"Capability has no durable approved connection",
								),
							);
				}),
				Effect.mapError(
					RepositoryError("Approved capability connection could not be read"),
				),
			);

	const RecordSessionAction = (input: {
		readonly action: "start" | "reconnect" | "restart";
		readonly capability_id: string;
		readonly operation_id: string;
		readonly request_fingerprint: string;
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [existing] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (existing) {
						if (
							existing.kind !== `session_${input.action}` ||
							existing.capability_id !== input.capability_id ||
							existing.request_fingerprint !== input.request_fingerprint
						)
							return yield* MakeError(
								"conflict",
								"Capability session action id was reused",
							);
						return "duplicate" as const;
					}
					const approved = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(
							eq(MarketplaceCapabilityOperations.capability_id, input.capability_id),
						);
					if (
						!approved.some(
							(operation) =>
								operation.kind === "connect" &&
								operation.approval_decision === "approved",
						)
					)
						return yield* MakeError(
							"conflict",
							"Capability session action requires durable connect approval",
						);
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						capability_id: input.capability_id,
						created_at: now,
						kind: `session_${input.action}`,
						operation_id: input.operation_id,
						request_fingerprint: input.request_fingerprint,
						state: "approved",
						updated_at: now,
					});
					return "accepted" as const;
				}),
			)
			.pipe(
				Effect.mapError(RepositoryError("Capability session action could not be recorded")),
			);

	const ClaimSessionAction = (operation_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || !operation.kind.startsWith("session_"))
						return yield* MakeError(
							"conflict",
							"Capability session action was not recorded",
						);
					if (operation.state === "completed") return "completed" as const;
					if (operation.state === "executing") return "executing" as const;
					if (operation.state !== "approved")
						return yield* MakeError(
							"conflict",
							"Capability session action is not approved",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "executing", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return "claimed" as const;
				}),
			)
			.pipe(
				Effect.mapError(RepositoryError("Capability session action could not be claimed")),
			);

	const CompleteSessionAction = (input: {
		readonly action: "start" | "reconnect" | "restart";
		readonly detail: CapabilityDetail;
		readonly operation_id: string;
		readonly server_metadata?: Readonly<Record<string, string>>;
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilities)
						.set({
							health_json: JSON.stringify(input.detail.health),
							instructions: input.detail.server_instructions ?? null,
							lifecycle: input.detail.lifecycle,
							resources_json: JSON.stringify(input.detail.resources),
							...(input.server_metadata === undefined
								? {}
								: {
										raw_provider_metadata_json: JSON.stringify(
											input.server_metadata,
										),
									}),
							status: input.detail.status,
							tools_json: JSON.stringify(input.detail.tools),
							updated_at: now,
						})
						.where(eq(MarketplaceCapabilities.id, input.detail.id));
					const sequence = yield* Append(transaction, {
						capability_id: input.detail.id,
						health: input.detail.health,
						operation:
							input.action === "start"
								? "started"
								: input.action === "restart"
									? "restarted"
									: "reconnected",
						operation_id: input.operation_id,
						status: input.detail.status,
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
				Effect.tap(notifier.Publish),
				Effect.asVoid,
				Effect.mapError(
					RepositoryError("Capability session action could not be completed"),
				),
			);

	const RecordUninstall = (input: {
		readonly capability_id: string;
		readonly operation_id: string;
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [existing] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (existing) {
						if (
							existing.kind !== "uninstall" ||
							existing.capability_id !== input.capability_id
						)
							return yield* MakeError(
								"conflict",
								"Capability uninstall id was reused with different intent",
							);
						return "duplicate" as const;
					}
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						capability_id: input.capability_id,
						created_at: now,
						kind: "uninstall",
						operation_id: input.operation_id,
						request_fingerprint: input.capability_id,
						state: "approved",
						updated_at: now,
					});
					return "accepted" as const;
				}),
			)
			.pipe(
				Effect.mapError(
					RepositoryError("Capability uninstall request could not be persisted"),
				),
			);

	const ClaimUninstall = (operation_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "uninstall")
						return yield* MakeError(
							"conflict",
							"Capability uninstall claim has no matching request",
						);
					if (operation.state === "uninstalled") return "uninstalled" as const;
					if (operation.state === "closing") return "closing" as const;
					if (operation.state !== "approved")
						return yield* MakeError(
							"conflict",
							"Capability uninstall is not recoverable",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "closing", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return "claimed" as const;
				}),
			)
			.pipe(
				Effect.mapError(
					RepositoryError("Capability uninstall claim could not be persisted"),
				),
			);

	const CompleteUninstall = (operation_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "uninstall")
						return yield* MakeError(
							"conflict",
							"Capability uninstall completion has no matching claim",
						);
					if (operation.state === "uninstalled") return;
					if (operation.state !== "closing")
						return yield* MakeError(
							"conflict",
							"Capability uninstall completion is not recoverable",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilities)
						.set({
							enabled: false,
							lifecycle: "removed",
							removed_at: now,
							status: "removed",
							updated_at: now,
						})
						.where(eq(MarketplaceCapabilities.id, operation.capability_id));
					const sequence = yield* Append(transaction, {
						capability_id: operation.capability_id,
						operation: "uninstalled",
						operation_id,
						status: "removed",
					});
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ state: "uninstalled", updated_at: now })
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id));
					return sequence;
				}),
			)
			.pipe(
				Effect.tap((sequence) =>
					sequence === undefined ? Effect.void : notifier.Publish(sequence),
				),
				Effect.mapError(
					RepositoryError("Capability uninstall completion could not be persisted"),
				),
			);

	return {
		ClaimConnect,
		ClaimSessionAction,
		ClaimUninstall,
		CompleteSessionAction,
		CompleteUninstall,
		DecideConnect,
		ReadApprovedConnect,
		ReadConnectApproval,
		ReadConnectRequest,
		RecordConnectRequest,
		RecordSessionAction,
		RecordUninstall,
	};
});
