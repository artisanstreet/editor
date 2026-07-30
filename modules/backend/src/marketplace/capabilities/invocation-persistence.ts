import { eq } from "drizzle-orm";
import { Context, Effect, Schema } from "effect";

import type {
	CapabilityDetail,
	CapabilityInvocationMetadata,
	MarketplaceLedgerEvent,
} from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import {
	MarketplaceCapabilityArtifacts,
	MarketplaceCapabilityOperations,
} from "../../persistence/tables";
import type { RuntimeIdPrefix } from "../../runtime/metadata";
import type { CapabilityRepositoryError } from "./repository";

type InvocationTransaction = Parameters<Parameters<DatabaseClient["transaction"]>[0]>[0];

interface InvocationAppendInput {
	readonly artifact_id?: string;
	readonly capability_id: string;
	readonly invocation_status?: CapabilityInvocationMetadata["status"];
	readonly operation: MarketplaceLedgerEvent["operation"];
	readonly operation_id: string;
	readonly status: CapabilityDetail["status"];
	readonly tool_name?: string;
}

export class InvocationPersistenceContext extends Context.Service<
	InvocationPersistenceContext,
	{
		readonly Append: (
			transaction: InvocationTransaction,
			input: InvocationAppendInput,
		) => Effect.Effect<number, unknown>;
		readonly database: { readonly client: DatabaseClient };
		readonly MakeError: (
			code: CapabilityRepositoryError["code"],
			message: string,
		) => CapabilityRepositoryError;
		readonly IsError: (error: unknown) => error is CapabilityRepositoryError;
		readonly metadata: {
			readonly MakeId: (prefix: RuntimeIdPrefix) => Effect.Effect<string>;
			readonly Now: Effect.Effect<string>;
		};
		readonly notifier: {
			readonly Publish: (journal_sequence: number) => Effect.Effect<void>;
		};
	}
>()("Artisan/Marketplace/InvocationPersistenceContext") {}

export const MakeInvocationPersistence = Effect.gen(function* () {
	const { Append, database, IsError, MakeError, metadata, notifier } =
		yield* InvocationPersistenceContext;

	const RecordInvocation = (input: {
		readonly approval_fingerprint?: string;
		readonly approval_id?: string;
		readonly capability_id: string;
		readonly operation_id: string;
		readonly request_fingerprint: string;
		readonly status: CapabilityDetail["status"];
		readonly tool_name: string;
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
							existing.kind !== "invoke" ||
							existing.capability_id !== input.capability_id ||
							existing.request_fingerprint !== input.request_fingerprint ||
							existing.tool_name !== input.tool_name ||
							existing.approval_id !== (input.approval_id ?? null) ||
							existing.approval_fingerprint !== (input.approval_fingerprint ?? null)
						)
							return yield* MakeError(
								"conflict",
								"Capability invocation id was reused with different intent",
							);
						return { result: "duplicate" as const };
					}
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityOperations).values({
						approval_id: input.approval_id ?? null,
						approval_fingerprint: input.approval_fingerprint ?? null,
						capability_id: input.capability_id,
						created_at: now,
						kind: "invoke",
						operation_id: input.operation_id,
						request_fingerprint: input.request_fingerprint,
						state: "awaiting_approval",
						tool_name: input.tool_name,
						updated_at: now,
					});
					const sequence = yield* Append(transaction, {
						capability_id: input.capability_id,
						invocation_status: "requested",
						operation: "invoked",
						operation_id: `${input.operation_id}:requested`,
						status: input.status,
						tool_name: input.tool_name,
					});
					return { result: "accepted" as const, sequence };
				}),
			)
			.pipe(
				Effect.tap(({ sequence }) =>
					sequence === undefined ? Effect.void : notifier.Publish(sequence),
				),
				Effect.map(({ result }) => result),
				Effect.mapError(() =>
					MakeError("invariant", "Capability invocation request could not be persisted"),
				),
			);

	const DecideInvocation = (input: {
		readonly approval_fingerprint?: string;
		readonly approval_id?: string;
		readonly approved: boolean;
		readonly operation_id: string;
		readonly status: CapabilityDetail["status"];
	}) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, input.operation_id))
						.limit(1);
					if (!operation || operation.kind !== "invoke")
						return yield* MakeError(
							"conflict",
							"Capability invocation approval has no matching request",
						);
					if (
						operation.approval_id !== (input.approval_id ?? null) ||
						operation.approval_fingerprint !== (input.approval_fingerprint ?? null)
					)
						return yield* MakeError(
							"conflict",
							"Capability invocation approval does not match its request",
						);
					if (operation.state === "completed") return { result: "completed" as const };
					const state: "approved" | "denied" = input.approved ? "approved" : "denied";
					if (operation.state === state) return { result: "duplicate" as const };
					if (operation.state !== "awaiting_approval")
						return yield* MakeError(
							"conflict",
							"Capability invocation approval was already decided",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({ approval_decision: state, state, updated_at: now })
						.where(
							eq(MarketplaceCapabilityOperations.operation_id, input.operation_id),
						);
					const sequence = yield* Append(transaction, {
						capability_id: operation.capability_id,
						invocation_status: input.approved ? "approved" : "denied",
						operation: "invoked",
						operation_id: `${input.operation_id}:decision`,
						status: input.status,
						...(operation.tool_name === null ? {} : { tool_name: operation.tool_name }),
					});
					return { result: state, sequence };
				}),
			)
			.pipe(
				Effect.tap(({ sequence }) =>
					sequence === undefined ? Effect.void : notifier.Publish(sequence),
				),
				Effect.map(({ result }) => result),
				Effect.mapError((error) =>
					IsError(error)
						? error
						: MakeError(
								"invariant",
								"Capability invocation approval could not be persisted",
							),
				),
			);

	const ReadInvocationApproval = (approval_id: string) =>
		database.client
			.select()
			.from(MarketplaceCapabilityOperations)
			.where(eq(MarketplaceCapabilityOperations.approval_id, approval_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([operation]) =>
					!operation ||
					operation.kind !== "invoke" ||
					operation.approval_fingerprint === null ||
					operation.tool_name === null
						? Effect.fail(
								MakeError(
									"not_found",
									"Capability invocation approval was not found",
								),
							)
						: Effect.succeed({
								approval_fingerprint: operation.approval_fingerprint,
								capability_id: operation.capability_id,
								operation_id: operation.operation_id,
								request_fingerprint: operation.request_fingerprint,
								tool_name: operation.tool_name,
							}),
				),
				Effect.mapError((error) =>
					IsError(error)
						? error
						: MakeError(
								"invariant",
								"Capability invocation approval could not be read",
							),
				),
			);

	const ClaimInvocation = (operation_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [operation] = yield* transaction
						.select()
						.from(MarketplaceCapabilityOperations)
						.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
						.limit(1);
					if (!operation || operation.kind !== "invoke")
						return yield* MakeError(
							"conflict",
							"Capability invocation claim has no matching request",
						);
					if (operation.state === "completed") return "completed" as const;
					if (operation.state === "denied") return "denied" as const;
					if (operation.state === "executing") return "executing" as const;
					if (operation.state !== "approved")
						return yield* MakeError(
							"conflict",
							"Capability invocation is not approved",
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
				Effect.mapError((error) =>
					IsError(error)
						? error
						: MakeError(
								"invariant",
								"Capability invocation claim could not be persisted",
							),
				),
			);

	const CompleteInvocation = (input: {
		readonly approval_required: boolean;
		readonly artifact_id: string;
		readonly capability_id: string;
		readonly operation_id: string;
		readonly result_json: string;
		readonly status: CapabilityDetail["status"];
		readonly tool_name: string;
	}) =>
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
						operation.kind !== "invoke" ||
						operation.capability_id !== input.capability_id ||
						operation.tool_name !== input.tool_name
					)
						return yield* MakeError(
							"conflict",
							"Capability completion has no matching invocation",
						);
					if (
						operation.state === "completed" &&
						operation.artifact_id !== input.artifact_id
					)
						return yield* MakeError(
							"conflict",
							"Capability invocation completion changed its artifact",
						);
					if (operation.state === "completed")
						return {
							metadata: {
								approval_required: input.approval_required,
								capability_id: input.capability_id,
								invocation_id: input.operation_id,
								result_artifact_id: input.artifact_id,
								status: "completed",
								tool_name: input.tool_name,
							} satisfies CapabilityInvocationMetadata,
						};
					if (operation.state !== "executing")
						return yield* MakeError(
							"conflict",
							"Capability invocation is not executing",
						);
					const now = yield* metadata.Now;
					yield* transaction.insert(MarketplaceCapabilityArtifacts).values({
						artifact_id: input.artifact_id,
						capability_id: input.capability_id,
						created_at: now,
						operation_id: input.operation_id,
						result_json: input.result_json,
						tool_name: input.tool_name,
					});
					const sequence = yield* Append(transaction, {
						artifact_id: input.artifact_id,
						capability_id: input.capability_id,
						invocation_status: "completed",
						operation: "invoked",
						operation_id: `${input.operation_id}:completed`,
						status: input.status,
						tool_name: input.tool_name,
					});
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({
							artifact_id: input.artifact_id,
							state: "completed",
							updated_at: now,
						})
						.where(
							eq(MarketplaceCapabilityOperations.operation_id, input.operation_id),
						);
					return {
						metadata: {
							approval_required: input.approval_required,
							capability_id: input.capability_id,
							invocation_id: input.operation_id,
							result_artifact_id: input.artifact_id,
							status: "completed",
							tool_name: input.tool_name,
						} satisfies CapabilityInvocationMetadata,
						sequence,
					};
				}),
			)
			.pipe(
				Effect.tap(({ sequence }) =>
					sequence === undefined ? Effect.void : notifier.Publish(sequence),
				),
				Effect.map(({ metadata: invocation_metadata }) => invocation_metadata),
			);

	const FailInvocation = (input: {
		readonly approval_required: boolean;
		readonly capability_id: string;
		readonly operation_id: string;
		readonly status: CapabilityDetail["status"];
		readonly tool_name: string;
	}) =>
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
						operation.kind !== "invoke" ||
						operation.capability_id !== input.capability_id ||
						operation.tool_name !== input.tool_name
					)
						return yield* MakeError(
							"conflict",
							"Capability failure has no matching invocation",
						);
					const invocation_metadata = {
						approval_required: input.approval_required,
						capability_id: input.capability_id,
						invocation_id: input.operation_id,
						status: "failed",
						tool_name: input.tool_name,
					} satisfies CapabilityInvocationMetadata;
					if (operation.state === "failed") return { metadata: invocation_metadata };
					if (operation.state !== "executing")
						return yield* MakeError(
							"conflict",
							"Capability invocation is not executing",
						);
					const now = yield* metadata.Now;
					yield* transaction
						.update(MarketplaceCapabilityOperations)
						.set({
							failure_code: "tool_call_failed",
							state: "failed",
							updated_at: now,
						})
						.where(
							eq(MarketplaceCapabilityOperations.operation_id, input.operation_id),
						);
					const sequence = yield* Append(transaction, {
						capability_id: input.capability_id,
						invocation_status: "failed",
						operation: "invoked",
						operation_id: `${input.operation_id}:failed`,
						status: input.status,
						tool_name: input.tool_name,
					});
					return { metadata: invocation_metadata, sequence };
				}),
			)
			.pipe(
				Effect.tap(({ sequence }) =>
					sequence === undefined ? Effect.void : notifier.Publish(sequence),
				),
				Effect.map(({ metadata: invocation_metadata }) => invocation_metadata),
			);

	const ReadInvocationArtifact = (artifact_id: string) =>
		database.client
			.select()
			.from(MarketplaceCapabilityArtifacts)
			.where(eq(MarketplaceCapabilityArtifacts.artifact_id, artifact_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([artifact]) =>
					artifact === undefined
						? Effect.fail(
								MakeError(
									"not_found",
									"Capability invocation artifact was not found",
								),
							)
						: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								artifact.result_json,
							).pipe(
								Effect.mapError(() =>
									MakeError(
										"invariant",
										"Capability invocation artifact is corrupt",
									),
								),
							),
				),
				Effect.mapError((error) =>
					IsError(error)
						? error
						: MakeError(
								"invariant",
								"Capability invocation artifact could not be read",
							),
				),
			);

	return {
		ClaimInvocation,
		CompleteInvocation,
		DecideInvocation,
		FailInvocation,
		ReadInvocationApproval,
		ReadInvocationArtifact,
		RecordInvocation,
	};
});
