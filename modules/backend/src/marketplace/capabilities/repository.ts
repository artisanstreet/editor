import { asc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	CapabilityDetail,
	CapabilityHealth,
	CapabilityInvocationMetadata,
	CapabilitySummary,
	MarketplaceLedgerEvent,
	type MarketplaceBrowseQuery,
	type MarketplaceScope,
	ProviderSyncState,
} from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceCapabilities,
	MarketplaceCapabilityMirrors,
	MarketplaceCapabilityOperations,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import { settings_scope_id, settings_stream_id } from "../../settings/internal-scope";
import {
	type DriftPersistence,
	DriftPersistenceContext,
	MakeDriftPersistence,
} from "./drift-persistence";
import { InvocationPersistenceContext, MakeInvocationPersistence } from "./invocation-persistence";
import {
	type LifecyclePersistence,
	LifecyclePersistenceContext,
	MakeLifecyclePersistence,
} from "./lifecycle-persistence";
import type { OAuthTokenStatus } from "./oauth";

export const marketplace_capability_thread_id = settings_scope_id("marketplace-capabilities");
const marketplace_capability_stream_id = settings_stream_id("marketplace-capabilities");

const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(left.kind === "workspace" &&
			right.kind === "workspace" &&
			left.workspace_id === right.workspace_id) ||
		(left.kind === "project" &&
			right.kind === "project" &&
			left.project_id === right.project_id));

type CapabilitySummaryRow = Pick<
	typeof MarketplaceCapabilities.$inferSelect,
	| "compatibility_json"
	| "display_name"
	| "enabled"
	| "health_json"
	| "id"
	| "lifecycle"
	| "scope_json"
	| "status"
	| "transport_json"
>;

export class CapabilityRepositoryError extends Data.TaggedError("CapabilityRepositoryError")<{
	readonly code: "conflict" | "invariant" | "not_found";
	readonly message: string;
}> {}

/** Decode SQLite JSON before it reaches the canonical capability model. */
const Parse = <A>(schema: Schema.Codec<A, unknown, never, never>, value: string, field: string) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema)),
		Effect.mapError(
			() =>
				new CapabilityRepositoryError({
					code: "invariant",
					message: `Capability ${field} is corrupt`,
				}),
		),
	);

const DetailFrom = (
	row: typeof MarketplaceCapabilities.$inferSelect,
	sync: ReadonlyArray<ProviderSyncState>,
) =>
	Effect.gen(function* () {
		const detail = yield* Schema.decodeUnknownEffect(CapabilityDetail)({
			auth: yield* Parse(CapabilityDetail.fields.auth, row.auth_json, "auth"),
			compatibility: yield* Parse(
				CapabilityDetail.fields.compatibility,
				row.compatibility_json,
				"compatibility",
			),
			display_name: row.display_name,
			enabled: row.enabled,
			health: yield* Parse(CapabilityHealth, row.health_json, "health"),
			id: row.id,
			lifecycle: row.lifecycle,
			permissions: yield* Parse(
				CapabilityDetail.fields.permissions,
				row.permissions_json,
				"permissions",
			),
			policy: yield* Parse(CapabilityDetail.fields.policy, row.policy_json, "policy"),
			resources: yield* Parse(
				CapabilityDetail.fields.resources,
				row.resources_json,
				"resources",
			),
			scope: yield* Parse(CapabilityDetail.fields.scope, row.scope_json, "scope"),
			server_instructions: row.instructions ?? undefined,
			...(row.raw_provider_metadata_json === null
				? {}
				: {
						server_metadata: yield* Parse(
							Schema.Record(Schema.String, Schema.String),
							row.raw_provider_metadata_json,
							"server metadata",
						),
					}),
			source: yield* Parse(CapabilityDetail.fields.source, row.source_json, "source"),
			status: row.status,
			sync,
			tools: yield* Parse(CapabilityDetail.fields.tools, row.tools_json, "tools"),
			transport: yield* Parse(
				CapabilityDetail.fields.transport,
				row.transport_json,
				"transport",
			),
			trust: row.trust,
			...(row.removed_at === null ? {} : { removed_at: row.removed_at }),
		});
		return detail;
	});

export class CapabilityRepository extends Context.Service<
	CapabilityRepository,
	DriftPersistence &
		LifecyclePersistence & {
			readonly Create: (input: {
				readonly detail: CapabilityDetail;
				readonly operation_id: string;
				readonly request_fingerprint: string;
				readonly server_metadata?: Readonly<Record<string, string>>;
			}) => Effect.Effect<void, CapabilityRepositoryError>;
			readonly RecordInvocation: (input: {
				readonly approval_fingerprint?: string;
				readonly approval_id?: string;
				readonly capability_id: string;
				readonly operation_id: string;
				readonly request_fingerprint: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name: string;
			}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
			readonly DecideInvocation: (input: {
				readonly approval_fingerprint?: string;
				readonly approval_id?: string;
				readonly approved: boolean;
				readonly operation_id: string;
				readonly status: CapabilityDetail["status"];
			}) => Effect.Effect<"approved" | "denied" | "completed" | "duplicate", unknown>;
			readonly ReadInvocationApproval: (approval_id: string) => Effect.Effect<
				{
					readonly approval_fingerprint: string;
					readonly capability_id: string;
					readonly operation_id: string;
					readonly request_fingerprint: string;
					readonly tool_name: string;
				},
				CapabilityRepositoryError
			>;
			readonly ClaimInvocation: (
				operation_id: string,
			) => Effect.Effect<"claimed" | "completed" | "denied" | "executing", unknown>;
			readonly CompleteInvocation: (input: {
				readonly approval_required: boolean;
				readonly artifact_id: string;
				readonly capability_id: string;
				readonly operation_id: string;
				readonly result_json: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name: string;
			}) => Effect.Effect<CapabilityInvocationMetadata, unknown>;
			readonly FailInvocation: (input: {
				readonly approval_required: boolean;
				readonly capability_id: string;
				readonly operation_id: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name: string;
			}) => Effect.Effect<CapabilityInvocationMetadata, unknown>;
			readonly ReadInvocationArtifact: (
				artifact_id: string,
			) => Effect.Effect<unknown, CapabilityRepositoryError>;
			readonly RecordOAuthOperation: (input: {
				readonly capability_id: string;
				readonly kind: "oauth_begin" | "oauth_complete" | "oauth_refresh" | "oauth_revoke";
				readonly operation_id: string;
				readonly request_fingerprint: string;
			}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
			readonly ClaimOAuthOperation: (
				operation_id: string,
			) => Effect.Effect<"claimed" | "completed" | "executing", CapabilityRepositoryError>;
			/** Reads the side-effect-free browser continuation retained for an exact Begin retry. */
			readonly ReadOAuthBeginResult: (
				operation_id: string,
			) => Effect.Effect<
				{ readonly authorization_url: string; readonly state: string },
				CapabilityRepositoryError
			>;
			readonly CompleteOAuthOperation: (input: {
				readonly begin_result?: {
					readonly authorization_url: string;
					readonly state: string;
				};
				readonly operation:
					| "oauth_started"
					| "oauth_completed"
					| "oauth_refreshed"
					| "oauth_revoked";
				readonly operation_id: string;
				readonly state_fingerprint?: string;
				readonly status: CapabilityDetail["status"];
				/** The opaque vault reference and lifecycle state returned by the OAuth boundary. */
				readonly token_status?: OAuthTokenStatus;
			}) => Effect.Effect<void, CapabilityRepositoryError>;
			readonly ReadDetail: (
				capability_id: string,
			) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
			readonly ReadSummaries: Effect.Effect<
				ReadonlyArray<CapabilitySummary>,
				CapabilityRepositoryError
			>;
			/** Reads filtered registry rows without loading capability details or provider mirrors. */
			readonly Browse: (
				query: MarketplaceBrowseQuery,
			) => Effect.Effect<ReadonlyArray<CapabilitySummary>, CapabilityRepositoryError>;
			readonly Transition: (input: {
				readonly capability_id: string;
				readonly enabled?: boolean;
				readonly health?: CapabilityHealth;
				readonly lifecycle?: CapabilityDetail["lifecycle"];
				readonly operation: MarketplaceLedgerEvent["operation"];
				readonly operation_id: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name?: string;
				readonly artifact_id?: string;
			}) => Effect.Effect<void, unknown>;
			readonly SetMirror: (input: {
				readonly capability_id: string;
				readonly state: ProviderSyncState;
			}) => Effect.Effect<void, unknown>;
		}
>()("Artisan/Marketplace/CapabilityRepository") {}

export const CapabilityRepositoryLive = Layer.effect(
	CapabilityRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		type CapabilityTransaction = Parameters<
			Parameters<typeof database.client.transaction>[0]
		>[0];
		const ReadDetail = (capability_id: string) =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select()
					.from(MarketplaceCapabilities)
					.where(eq(MarketplaceCapabilities.id, capability_id))
					.limit(1);
				if (!row)
					return yield* new CapabilityRepositoryError({
						code: "not_found",
						message: `Capability ${capability_id} was not found`,
					});
				const mirrors = yield* database.client
					.select()
					.from(MarketplaceCapabilityMirrors)
					.where(eq(MarketplaceCapabilityMirrors.capability_id, capability_id));
				const sync = yield* Effect.forEach(mirrors, (mirror) =>
					Schema.decodeUnknownEffect(ProviderSyncState)({
						engine_id: mirror.engine_id,
						status: mirror.status,
						updated_at: mirror.updated_at,
						...(mirror.last_error_code === null
							? {}
							: { last_error_code: mirror.last_error_code }),
						...(mirror.observed_revision === null
							? {}
							: { observed_revision: mirror.observed_revision }),
					}).pipe(
						Effect.mapError(
							() =>
								new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability mirror is corrupt",
								}),
						),
					),
				);
				return yield* DetailFrom(row, sync);
			}).pipe(
				Effect.mapError(
					(error: unknown): CapabilityRepositoryError =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability detail could not be read",
								}),
				),
			);
		const DecodeSummary = (row: CapabilitySummaryRow) =>
			Effect.all({
				compatibility: Parse(
					CapabilityDetail.fields.compatibility,
					row.compatibility_json,
					"compatibility",
				),
				health: Parse(CapabilityHealth, row.health_json, "health"),
				scope: Parse(CapabilityDetail.fields.scope, row.scope_json, "scope"),
				transport: Parse(
					CapabilityDetail.fields.transport,
					row.transport_json,
					"transport",
				),
			}).pipe(
				Effect.flatMap(({ compatibility, health, scope, transport }) =>
					Schema.decodeUnknownEffect(CapabilitySummary)({
						display_name: row.display_name,
						enabled: row.enabled,
						health,
						id: row.id,
						lifecycle: row.lifecycle,
						scope,
						status: row.status,
						transport_kind: transport.kind,
					}).pipe(Effect.map((summary) => ({ compatibility, summary }))),
				),
				Effect.mapError(
					() =>
						new CapabilityRepositoryError({
							code: "invariant",
							message: "Capability summary is corrupt",
						}),
				),
			);
		const ReadSummaryRows = database.client
			.select({
				compatibility_json: MarketplaceCapabilities.compatibility_json,
				display_name: MarketplaceCapabilities.display_name,
				enabled: MarketplaceCapabilities.enabled,
				health_json: MarketplaceCapabilities.health_json,
				id: MarketplaceCapabilities.id,
				lifecycle: MarketplaceCapabilities.lifecycle,
				scope_json: MarketplaceCapabilities.scope_json,
				status: MarketplaceCapabilities.status,
				transport_json: MarketplaceCapabilities.transport_json,
			})
			.from(MarketplaceCapabilities)
			.orderBy(asc(MarketplaceCapabilities.display_name))
			.pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeSummary, { concurrency: 16 })),
				Effect.mapError(
					(error: unknown): CapabilityRepositoryError =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability summaries could not be read",
								}),
				),
			);
		const ReadSummaries = ReadSummaryRows.pipe(
			Effect.map((rows) => rows.map(({ summary }) => summary)),
		);
		const Browse = (query: MarketplaceBrowseQuery) =>
			ReadSummaryRows.pipe(
				Effect.map((rows) =>
					rows
						.filter(
							({ compatibility, summary }) =>
								(query.compatibility_engine_id === undefined ||
									compatibility.some(
										(entry) =>
											entry.engine_id === query.compatibility_engine_id,
									)) &&
								(query.category === undefined || query.category === "capability") &&
								(query.enabled === undefined ||
									summary.enabled === query.enabled) &&
								(query.status === undefined || summary.status === query.status) &&
								(query.scope === undefined ||
									ScopeMatches(summary.scope, query.scope)) &&
								(query.text === undefined ||
									summary.display_name
										.toLocaleLowerCase()
										.includes(query.text.toLocaleLowerCase())),
						)
						.map(({ summary }) => summary),
				),
			);
		const Append = (
			transaction: CapabilityTransaction,
			input: {
				readonly capability_id: string;
				readonly operation: MarketplaceLedgerEvent["operation"];
				readonly operation_id: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name?: string;
				readonly artifact_id?: string;
				readonly health?: CapabilityHealth;
				readonly invocation_status?: CapabilityInvocationMetadata["status"];
			},
		) =>
			Effect.gen(function* () {
				const occurred_at = yield* metadata.Now;
				const event_id = yield* metadata.MakeId("event");
				const [stream] = yield* transaction
					.select()
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, marketplace_capability_stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const payload = {
					item_id: input.capability_id,
					item_kind: "capability" as const,
					operation: input.operation,
					status: input.status,
					type: "marketplace.lifecycle" as const,
					...(input.tool_name === undefined ? {} : { tool_name: input.tool_name }),
					...(input.artifact_id === undefined ? {} : { artifact_id: input.artifact_id }),
					...(input.invocation_status === undefined
						? {}
						: { invocation_status: input.invocation_status }),
					...(input.health === undefined
						? {}
						: { capability_health: input.health.status }),
				};
				yield* Schema.decodeUnknownEffect(MarketplaceLedgerEvent)(payload).pipe(
					Effect.mapError(
						() =>
							new CapabilityRepositoryError({
								code: "invariant",
								message: "Capability lifecycle event is invalid",
							}),
					),
				);
				const payload_json = JSON.stringify(payload);
				const [previous] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, input.operation_id))
					.limit(1);
				if (previous) {
					if (
						previous.payload_json !== payload_json ||
						previous.thread_id !== marketplace_capability_thread_id
					)
						return yield* new CapabilityRepositoryError({
							code: "conflict",
							message: "Capability operation id was reused with different intent",
						});
					const [existing_event] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.where(eq(JournalEvents.correlation_id, input.operation_id))
						.limit(1);
					if (!existing_event || existing_event.journal_sequence <= 0)
						return yield* new CapabilityRepositoryError({
							code: "invariant",
							message: "Capability lifecycle command has no positive journal event",
						});
					return existing_event.journal_sequence;
				}
				if (stream)
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: sequence })
						.where(eq(EventStreams.stream_id, marketplace_capability_stream_id));
				else
					yield* transaction.insert(EventStreams).values({
						stream_id: marketplace_capability_stream_id,
						last_sequence: sequence,
					});
				yield* transaction.insert(JournalCommands).values({
					accepted_at: occurred_at,
					message_id: input.operation_id,
					origin: "backend",
					payload_json,
					payload_type: "marketplace.capability.lifecycle",
					schema_version: 1,
					sent_at: occurred_at,
					status: "accepted",
					thread_id: marketplace_capability_thread_id,
				});
				const [event] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id: input.operation_id,
						correlation_id: input.operation_id,
						event_id,
						event_type: payload.type,
						occurred_at,
						origin: "backend",
						payload_json,
						schema_version: 1,
						stream_id: marketplace_capability_stream_id,
						stream_sequence: sequence,
						thread_id: marketplace_capability_thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });
				if (!event || event.journal_sequence <= 0)
					return yield* new CapabilityRepositoryError({
						code: "invariant",
						message: "Capability lifecycle event has no positive journal sequence",
					});
				return event.journal_sequence;
			});
		const invocation_persistence = yield* MakeInvocationPersistence.pipe(
			Effect.provideService(
				InvocationPersistenceContext,
				InvocationPersistenceContext.of({
					Append,
					database,
					IsError: (error: unknown): error is CapabilityRepositoryError =>
						error instanceof CapabilityRepositoryError,
					MakeError: (code, message) => new CapabilityRepositoryError({ code, message }),
					metadata,
					notifier,
				}),
			),
		);
		const lifecycle_persistence = yield* MakeLifecyclePersistence.pipe(
			Effect.provideService(
				LifecyclePersistenceContext,
				LifecyclePersistenceContext.of({
					Append,
					database,
					DecodeDetail: (value, field) => Parse(CapabilityDetail, value, field),
					IsError: (error: unknown): error is CapabilityRepositoryError =>
						error instanceof CapabilityRepositoryError,
					MakeError: (code, message) => new CapabilityRepositoryError({ code, message }),
					metadata,
					notifier,
				}),
			),
		);
		const Create = (input: {
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly request_fingerprint: string;
			readonly server_metadata?: Readonly<Record<string, string>>;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (
							!operation ||
							operation.state !== "connecting" ||
							operation.capability_id !== input.detail.id
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability connection is not durably approved",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.insert(MarketplaceCapabilities)
							.values({
								id: input.detail.id,
								display_name: input.detail.display_name,
								source_json: JSON.stringify(input.detail.source),
								transport_json: JSON.stringify(input.detail.transport),
								auth_json: JSON.stringify(input.detail.auth),
								scope_json: JSON.stringify(input.detail.scope),
								permissions_json: JSON.stringify(input.detail.permissions),
								compatibility_json: JSON.stringify(input.detail.compatibility),
								tools_json: JSON.stringify(input.detail.tools),
								resources_json: JSON.stringify(input.detail.resources),
								instructions: input.detail.server_instructions ?? null,
								policy_json: JSON.stringify(input.detail.policy),
								trust: input.detail.trust,
								enabled: input.detail.enabled,
								status: input.detail.status,
								lifecycle: input.detail.lifecycle,
								health_json: JSON.stringify(input.detail.health),
								raw_provider_metadata_json:
									input.server_metadata === undefined
										? null
										: JSON.stringify(input.server_metadata),
								removed_at: input.detail.removed_at ?? null,
								created_at: now,
								updated_at: now,
							})
							.onConflictDoNothing();
						const journal_sequence = yield* Append(transaction, {
							capability_id: input.detail.id,
							operation: "connected",
							operation_id: input.operation_id,
							status: input.detail.status,
							health: input.detail.health,
						});
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ state: "connected", updated_at: now })
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						return journal_sequence;
					}),
				)
				.pipe(
					Effect.tap((journal_sequence) => notifier.Publish(journal_sequence)),
					Effect.mapError(
						() =>
							new CapabilityRepositoryError({
								code: "invariant",
								message: "Capability creation could not be persisted",
							}),
					),
				);
		const {
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
		} = lifecycle_persistence;
		const drift_persistence = yield* MakeDriftPersistence.pipe(
			Effect.provideService(
				DriftPersistenceContext,
				DriftPersistenceContext.of({
					Append,
					database,
					IsError: (error: unknown): error is CapabilityRepositoryError =>
						error instanceof CapabilityRepositoryError,
					MakeError: (code, message) => new CapabilityRepositoryError({ code, message }),
					metadata,
					notifier,
				}),
			),
		);
		const RecordOAuthOperation = (input: {
			readonly capability_id: string;
			readonly kind: "oauth_begin" | "oauth_complete" | "oauth_refresh" | "oauth_revoke";
			readonly operation_id: string;
			readonly request_fingerprint: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (existing) {
							if (
								existing.capability_id !== input.capability_id ||
								existing.kind !== input.kind ||
								existing.request_fingerprint !== input.request_fingerprint
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message: "OAuth operation id was reused with different intent",
								});
							return "duplicate" as const;
						}
						const now = yield* metadata.Now;
						yield* transaction.insert(MarketplaceCapabilityOperations).values({
							capability_id: input.capability_id,
							created_at: now,
							kind: input.kind,
							operation_id: input.operation_id,
							request_fingerprint: input.request_fingerprint,
							state: "pending",
							updated_at: now,
						});
						return "accepted" as const;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "OAuth operation could not be persisted",
								}),
					),
				);
		const ClaimOAuthOperation = (operation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
							.limit(1);
						if (!operation || !operation.kind.startsWith("oauth_"))
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "OAuth claim has no matching intent",
							});
						if (operation.state === "completed") return "completed" as const;
						if (operation.state === "executing") return "executing" as const;
						if (operation.state !== "pending")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "OAuth operation is not claimable",
							});
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
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "OAuth claim could not be persisted",
								}),
					),
				);
		const ReadOAuthBeginResult = (operation_id: string) =>
			database.client
				.select({
					kind: MarketplaceCapabilityOperations.kind,
					preview_json: MarketplaceCapabilityOperations.preview_json,
					state: MarketplaceCapabilityOperations.state,
				})
				.from(MarketplaceCapabilityOperations)
				.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([operation]) =>
						operation?.kind === "oauth_begin" &&
						operation.state === "completed" &&
						operation.preview_json !== null
							? Parse(
									Schema.Struct({
										authorization_url: Schema.String,
										state: Schema.String,
									}),
									operation.preview_json,
									"OAuth begin result",
								)
							: Effect.fail(
									new CapabilityRepositoryError({
										code: "conflict",
										message:
											"OAuth begin has no recoverable browser continuation",
									}),
								),
					),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "OAuth begin result could not be read",
								}),
					),
				);
		const CompleteOAuthOperation = (input: {
			readonly begin_result?: { readonly authorization_url: string; readonly state: string };
			readonly operation:
				| "oauth_started"
				| "oauth_completed"
				| "oauth_refreshed"
				| "oauth_revoked";
			readonly operation_id: string;
			readonly state_fingerprint?: string;
			readonly status: CapabilityDetail["status"];
			readonly token_status?: OAuthTokenStatus;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (!operation || !operation.kind.startsWith("oauth_"))
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "OAuth completion has no matching claim",
							});
						if (operation.state === "completed") return;
						if (operation.state !== "executing")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "OAuth completion is not recoverable",
							});
						const now = yield* metadata.Now;
						if (input.token_status !== undefined) {
							const [capability] = yield* transaction
								.select({ auth_json: MarketplaceCapabilities.auth_json })
								.from(MarketplaceCapabilities)
								.where(eq(MarketplaceCapabilities.id, operation.capability_id))
								.limit(1);
							if (capability === undefined)
								return yield* new CapabilityRepositoryError({
									code: "not_found",
									message: "OAuth completion capability was not found",
								});
							const auth = yield* Parse(
								CapabilityDetail.fields.auth,
								capability.auth_json,
								"auth",
							);
							if (auth.kind !== "oauth")
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message: "OAuth completion capability does not use OAuth",
								});
							const next_auth =
								input.token_status.state === "active"
									? {
											...auth,
											token_ref:
												input.token_status.secret_reference === undefined
													? auth.token_ref
													: input.token_status.secret_reference,
											token_status: "authorized" as const,
										}
									: input.token_status.state === "expired"
										? { ...auth, token_status: "expired" as const }
										: {
												authorization_url: auth.authorization_url,
												kind: "oauth" as const,
												provider: auth.provider,
												scopes: auth.scopes,
												token_status: "not_started" as const,
											};
							yield* transaction
								.update(MarketplaceCapabilities)
								.set({ auth_json: JSON.stringify(next_auth), updated_at: now })
								.where(eq(MarketplaceCapabilities.id, operation.capability_id));
						}
						const sequence = yield* Append(transaction, {
							capability_id: operation.capability_id,
							operation: input.operation,
							operation_id: input.operation_id,
							status: input.status,
						});
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({
								preview_json:
									input.begin_result === undefined
										? (input.state_fingerprint ?? null)
										: JSON.stringify(input.begin_result),
								state: "completed",
								updated_at: now,
							})
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						return sequence;
					}),
				)
				.pipe(
					Effect.tap((sequence) =>
						sequence === undefined ? Effect.void : notifier.Publish(sequence),
					),
					Effect.asVoid,
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "OAuth completion could not be persisted",
								}),
					),
				);
		const Transition = (input: {
			readonly capability_id: string;
			readonly enabled?: boolean;
			readonly health?: CapabilityHealth;
			readonly lifecycle?: CapabilityDetail["lifecycle"];
			readonly operation: MarketplaceLedgerEvent["operation"];
			readonly operation_id: string;
			readonly status: CapabilityDetail["status"];
			readonly tool_name?: string;
			readonly artifact_id?: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const current = yield* ReadDetail(input.capability_id);
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilities)
							.set({
								enabled: input.enabled ?? current.enabled,
								health_json: JSON.stringify(input.health ?? current.health),
								lifecycle: input.lifecycle ?? current.lifecycle,
								status: input.status,
								removed_at:
									input.operation === "removed" ||
									input.operation === "uninstalled"
										? now
										: (current.removed_at ?? null),
								updated_at: now,
							})
							.where(eq(MarketplaceCapabilities.id, input.capability_id));
						return yield* Append(transaction, input);
					}),
				)
				.pipe(
					Effect.tap((journal_sequence) => notifier.Publish(journal_sequence)),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability transition could not be persisted",
								}),
					),
				);
		const SetMirror = (input: {
			readonly capability_id: string;
			readonly state: ProviderSyncState;
		}) =>
			database.client
				.insert(MarketplaceCapabilityMirrors)
				.values({
					capability_id: input.capability_id,
					engine_id: input.state.engine_id,
					status: input.state.status,
					observed_revision: input.state.observed_revision ?? null,
					last_error_code: input.state.last_error_code ?? null,
					updated_at: input.state.updated_at,
				})
				.onConflictDoUpdate({
					target: [
						MarketplaceCapabilityMirrors.capability_id,
						MarketplaceCapabilityMirrors.engine_id,
					],
					set: {
						status: input.state.status,
						observed_revision: input.state.observed_revision ?? null,
						last_error_code: input.state.last_error_code ?? null,
						updated_at: input.state.updated_at,
					},
				})
				.pipe(
					Effect.asVoid,
					Effect.mapError(
						() =>
							new CapabilityRepositoryError({
								code: "invariant",
								message: "Capability mirror could not be persisted",
							}),
					),
				);
		return {
			Create: (input: {
				readonly detail: CapabilityDetail;
				readonly operation_id: string;
				readonly request_fingerprint: string;
				readonly server_metadata?: Readonly<Record<string, string>>;
			}) => Create(input),
			RecordConnectRequest,
			ReadConnectRequest,
			ReadConnectApproval,
			RecordUninstall,
			ClaimUninstall,
			CompleteUninstall,
			...drift_persistence,
			DecideConnect,
			ClaimConnect,
			ReadApprovedConnect,
			RecordSessionAction,
			ClaimSessionAction,
			CompleteSessionAction,
			...invocation_persistence,
			RecordOAuthOperation,
			ClaimOAuthOperation,
			ReadOAuthBeginResult,
			CompleteOAuthOperation,
			ReadDetail,
			Browse,
			ReadSummaries,
			SetMirror: (input: {
				readonly capability_id: string;
				readonly state: ProviderSyncState;
			}) => SetMirror(input),
			Transition: (input: {
				readonly capability_id: string;
				readonly enabled?: boolean;
				readonly health?: CapabilityHealth;
				readonly lifecycle?: CapabilityDetail["lifecycle"];
				readonly operation: MarketplaceLedgerEvent["operation"];
				readonly operation_id: string;
				readonly status: CapabilityDetail["status"];
				readonly tool_name?: string;
				readonly artifact_id?: string;
			}) => Transition(input),
		};
	}),
);
