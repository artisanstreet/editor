import { asc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	CapabilityDetail,
	CapabilityHealth,
	CapabilityInvocationMetadata,
	CapabilitySummary,
	MarketplaceScope,
	MarketplaceLedgerEvent,
	ProviderSyncState,
} from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	MarketplaceCapabilities,
	MarketplaceCapabilityArtifacts,
	MarketplaceCapabilityMirrors,
	MarketplaceCapabilityOperations,
} from "../../persistence/schema";
import { RuntimeMetadata } from "../../runtime/runtime-metadata";
import { settings_scope_id, settings_stream_id } from "../../settings/internal-scope";
import type { OAuthTokenStatus } from "./oauth";

export const marketplace_capability_thread_id = settings_scope_id("marketplace-capabilities");
const marketplace_capability_stream_id = settings_stream_id("marketplace-capabilities");

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
	{
		readonly Create: (input: {
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly request_fingerprint: string;
			readonly server_metadata?: Readonly<Record<string, string>>;
		}) => Effect.Effect<void, CapabilityRepositoryError>;
		/** Persists the exact preview-bound request before a decision or transport action. */
		readonly RecordConnectRequest: (input: {
			readonly approval_fingerprint?: string;
			readonly approval_id?: string;
			readonly capability_id: string;
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly request_fingerprint: string;
		}) => Effect.Effect<"accepted" | "duplicate", unknown>;
		/** Returns the exact reviewed request so restart never trusts a new caller payload. */
		readonly ReadConnectRequest: (
			operation_id: string,
		) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
		readonly ReadConnectApproval: (approval_id: string) => Effect.Effect<
			{
				readonly approval_fingerprint: string;
				readonly detail: CapabilityDetail;
				readonly operation_id: string;
			},
			CapabilityRepositoryError
		>;
		/** Persists the approval result; only an approved record may be connected. */
		readonly DecideConnect: (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly approved: boolean;
		}) => Effect.Effect<
			"approved" | "connected" | "denied" | "duplicate",
			CapabilityRepositoryError
		>;
		readonly ClaimConnect: (
			operation_id: string,
		) => Effect.Effect<"claimed" | "connected" | "denied" | "connecting", unknown>;
		readonly ReadApprovedConnect: (
			capability_id: string,
		) => Effect.Effect<CapabilityDetail, CapabilityRepositoryError>;
		readonly RecordSessionAction: (input: {
			readonly action: "start" | "reconnect" | "restart";
			readonly capability_id: string;
			readonly operation_id: string;
			readonly request_fingerprint: string;
		}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
		readonly ClaimSessionAction: (
			operation_id: string,
		) => Effect.Effect<"claimed" | "completed" | "executing", CapabilityRepositoryError>;
		readonly CompleteSessionAction: (input: {
			readonly action: "start" | "reconnect" | "restart";
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly server_metadata?: Readonly<Record<string, string>>;
		}) => Effect.Effect<void, CapabilityRepositoryError>;
		readonly RecordUninstall: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<"accepted" | "duplicate", CapabilityRepositoryError>;
		readonly ClaimUninstall: (
			operation_id: string,
		) => Effect.Effect<"claimed" | "closing" | "uninstalled", CapabilityRepositoryError>;
		readonly CompleteUninstall: (
			operation_id: string,
		) => Effect.Effect<void, CapabilityRepositoryError>;
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
			readonly begin_result?: { readonly authorization_url: string; readonly state: string };
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
		const ReadSummaries = database.client
			.select()
			.from(MarketplaceCapabilities)
			.orderBy(asc(MarketplaceCapabilities.display_name))
			.pipe(
				Effect.flatMap((rows) =>
					Effect.forEach(rows, (row) =>
						Effect.all({
							health: Parse(CapabilityHealth, row.health_json, "health"),
							scope: Parse(CapabilityDetail.fields.scope, row.scope_json, "scope"),
							transport: Parse(
								CapabilityDetail.fields.transport,
								row.transport_json,
								"transport",
							),
						}).pipe(
							Effect.flatMap(({ health, scope, transport }) =>
								Schema.decodeUnknownEffect(CapabilitySummary)({
									display_name: row.display_name,
									enabled: row.enabled,
									health,
									id: row.id,
									lifecycle: row.lifecycle,
									scope,
									status: row.status,
									transport_kind: transport.kind,
								}),
							),
							Effect.mapError(
								() =>
									new CapabilityRepositoryError({
										code: "invariant",
										message: "Capability summary is corrupt",
									}),
							),
						),
					),
				),
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
								existing.approval_id !== (input.approval_id ?? null) ||
								existing.approval_fingerprint !==
									(input.approval_fingerprint ?? null) ||
								existing.detail_json !== JSON.stringify(input.detail) ||
								existing.request_fingerprint !== input.request_fingerprint ||
								existing.kind !== "connect"
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message:
										"Capability operation id was reused with different intent",
								});
							return "duplicate" as const;
						}
						const now = yield* metadata.Now;
						yield* transaction.insert(MarketplaceCapabilityOperations).values({
							approval_fingerprint: input.approval_fingerprint ?? null,
							approval_id: input.approval_id ?? null,
							capability_id: input.capability_id,
							created_at: now,
							detail_json: JSON.stringify(input.detail),
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
						() =>
							new CapabilityRepositoryError({
								code: "invariant",
								message: "Capability connection request could not be persisted",
							}),
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
							.where(
								eq(MarketplaceCapabilityOperations.approval_id, input.approval_id),
							)
							.limit(1);
						if (!operation || operation.kind !== "connect")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability approval has no matching request",
							});
						if (operation.approval_fingerprint !== input.approval_fingerprint)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message:
									"Capability approval fingerprint does not match reviewed detail",
							});
						const state: "approved" | "denied" = input.approved ? "approved" : "denied";
						if (operation.state === "connected" && input.approved)
							return "connected" as const;
						if (operation.state === state) return "duplicate" as const;
						if (operation.state !== "awaiting_approval")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability approval was already decided",
							});
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
				.pipe(
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability approval could not be persisted",
								}),
					),
				);
		const ReadConnectRequest = (operation_id: string) =>
			database.client
				.select()
				.from(MarketplaceCapabilityOperations)
				.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([operation]) => {
						if (
							!operation ||
							operation.kind !== "connect" ||
							operation.detail_json === null
						)
							return Effect.fail(
								new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability connection has no reviewed detail",
								}),
							);
						return Parse(
							CapabilityDetail,
							operation.detail_json,
							"reviewed connection detail",
						);
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability connection request could not be read",
								}),
					),
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
								new CapabilityRepositoryError({
									code: "not_found",
									message: "Capability approval request was not found",
								}),
							);
						return Parse(
							CapabilityDetail,
							operation.detail_json,
							"reviewed connection detail",
						).pipe(
							Effect.map((detail) => ({
								approval_fingerprint: operation.approval_fingerprint!,
								detail,
								operation_id: operation.operation_id,
							})),
						);
					}),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability approval request could not be read",
								}),
					),
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability connection claim has no matching request",
							});
						if (operation.state === "connected") return "connected" as const;
						if (operation.state === "denied") return "denied" as const;
						if (operation.state === "connecting") return "connecting" as const;
						if (operation.state !== "approved")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability connection is not approved",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ state: "connecting", updated_at: now })
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
									message: "Capability connection claim could not be persisted",
								}),
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
							? Parse(
									CapabilityDetail,
									approved.detail_json,
									"approved connect detail",
								)
							: Effect.fail(
									new CapabilityRepositoryError({
										code: "not_found",
										message: "Capability has no durable approved connection",
									}),
								);
					}),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Approved capability connection could not be read",
								}),
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== `session_${input.action}` ||
								existing.capability_id !== input.capability_id ||
								existing.request_fingerprint !== input.request_fingerprint
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message: "Capability session action id was reused",
								});
							return "duplicate" as const;
						}
						const approved = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(
								eq(
									MarketplaceCapabilityOperations.capability_id,
									input.capability_id,
								),
							);
						if (
							!approved.some(
								(operation) =>
									operation.kind === "connect" &&
									operation.approval_decision === "approved",
							)
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message:
									"Capability session action requires durable connect approval",
							});
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
					Effect.mapError(
						(error: unknown): CapabilityRepositoryError =>
							error instanceof CapabilityRepositoryError
								? error
								: new CapabilityRepositoryError({
										code: "invariant",
										message: "Capability session action could not be recorded",
									}),
					),
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability session action was not recorded",
							});
						if (operation.state === "completed") return "completed" as const;
						if (operation.state === "executing") return "executing" as const;
						if (operation.state !== "approved")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability session action is not approved",
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
									message: "Capability session action could not be claimed",
								}),
					),
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
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						return sequence;
					}),
				)
				.pipe(
					Effect.tap(notifier.Publish),
					Effect.asVoid,
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability session action could not be completed",
								}),
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== "uninstall" ||
								existing.capability_id !== input.capability_id
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message:
										"Capability uninstall id was reused with different intent",
								});
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
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability uninstall request could not be persisted",
								}),
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability uninstall claim has no matching request",
							});
						if (operation.state === "uninstalled") return "uninstalled" as const;
						if (operation.state === "closing") return "closing" as const;
						if (operation.state !== "approved")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability uninstall is not recoverable",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ state: "closing", updated_at: now })
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
									message: "Capability uninstall claim could not be persisted",
								}),
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability uninstall completion has no matching claim",
							});
						if (operation.state === "uninstalled") return;
						if (operation.state !== "closing")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability uninstall completion is not recoverable",
							});
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
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message:
										"Capability uninstall completion could not be persisted",
								}),
					),
				);
		const RecordDriftResolution = (input: {
			readonly approval_fingerprint?: string;
			readonly approval_id?: string;
			readonly action: "ignore" | "import" | "overwrite";
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const request_fingerprint = JSON.stringify({
							action: input.action,
							engine_id: input.engine_id,
							observed_revision: input.observed_revision,
							...(input.scope === undefined ? {} : { scope: input.scope }),
						});
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
								existing.kind !== "drift" ||
								existing.capability_id !== input.capability_id ||
								existing.request_fingerprint !== request_fingerprint ||
								existing.approval_id !== (input.approval_id ?? null) ||
								existing.approval_fingerprint !==
									(input.approval_fingerprint ?? null)
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message: "Capability drift id was reused with different intent",
								});
							return "duplicate" as const;
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
						return "accepted" as const;
					}),
				)
				.pipe(
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability drift action could not be persisted",
								}),
					),
				);
		const DecideDriftOverwrite = (input: {
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
							.where(
								eq(MarketplaceCapabilityOperations.approval_id, input.approval_id),
							)
							.limit(1);
						if (
							!operation ||
							operation.kind !== "drift" ||
							operation.approval_fingerprint !== input.approval_fingerprint
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Drift approval does not match its reviewed request",
							});
						if (operation.state === "completed" && input.approved)
							return "completed" as const;
						const state: "approved" | "denied" = input.approved ? "approved" : "denied";
						if (operation.state === state) return "duplicate" as const;
						if (operation.state !== "awaiting_approval")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Drift approval was already decided",
							});
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
				.pipe(
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Drift approval could not be persisted",
								}),
					),
				);
		const ReadDriftApproval = (approval_id: string) =>
			database.client
				.select()
				.from(MarketplaceCapabilityOperations)
				.where(eq(MarketplaceCapabilityOperations.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([operation]) => {
						if (!operation || operation.kind !== "drift")
							return Effect.fail(
								new CapabilityRepositoryError({
									code: "not_found",
									message: "Drift approval was not found",
								}),
							);
						return Schema.decodeUnknownEffect(
							Schema.Struct({
								action: Schema.Literal("overwrite"),
								engine_id: Schema.String,
								observed_revision: Schema.String,
								scope: MarketplaceScope,
							}),
						)(JSON.parse(operation.request_fingerprint)).pipe(
							Effect.map((intent) => ({
								capability_id: operation.capability_id,
								engine_id: intent.engine_id,
								observed_revision: intent.observed_revision,
								operation_id: operation.operation_id,
								scope: intent.scope,
							})),
							Effect.mapError(
								() =>
									new CapabilityRepositoryError({
										code: "invariant",
										message: "Drift approval intent is corrupt",
									}),
							),
						);
					}),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Drift approval could not be read",
								}),
					),
				);
		const ClaimDriftOverwrite = (operation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
							.limit(1);
						if (!operation || operation.kind !== "drift")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability drift claim has no matching request",
							});
						if (operation.state === "completed") return "completed" as const;
						if (operation.state === "writing") return "writing" as const;
						if (operation.state !== "approved")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability drift overwrite is not approved",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ state: "writing", updated_at: now })
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
									message: "Capability drift claim could not be persisted",
								}),
					),
				);
		const CompleteDriftResolution = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly state: ProviderSyncState;
			readonly status: CapabilityDetail["status"];
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						yield* transaction
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
							});
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
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						return sequence;
					}),
				)
				.pipe(
					Effect.tap((sequence) => notifier.Publish(sequence)),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability drift completion could not be persisted",
								}),
					),
				);
		const RecordProviderSync = (input: {
			readonly capability_id: string;
			readonly engine_id: string;
			readonly operation_id: string;
		}) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const fingerprint = JSON.stringify({ engine_id: input.engine_id });
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
								existing.kind !== "sync" ||
								existing.capability_id !== input.capability_id ||
								existing.request_fingerprint !== fingerprint
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message: "Provider sync id was reused with different intent",
								});
							return "duplicate" as const;
						}
						const now = yield* metadata.Now;
						yield* transaction.insert(MarketplaceCapabilityOperations).values({
							capability_id: input.capability_id,
							created_at: now,
							kind: "sync",
							operation_id: input.operation_id,
							request_fingerprint: fingerprint,
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
									message: "Provider sync request could not be persisted",
								}),
					),
				);
		const ClaimProviderSync = (operation_id: string) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [operation] = yield* transaction
							.select()
							.from(MarketplaceCapabilityOperations)
							.where(eq(MarketplaceCapabilityOperations.operation_id, operation_id))
							.limit(1);
						if (!operation || operation.kind !== "sync")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Provider sync claim has no request",
							});
						if (operation.state === "completed") return "completed" as const;
						if (operation.state === "syncing") return "syncing" as const;
						if (operation.state !== "pending")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Provider sync is not claimable",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ state: "syncing", updated_at: now })
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
									message: "Provider sync claim could not be persisted",
								}),
					),
				);
		const CompleteProviderSync = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly state: ProviderSyncState;
			readonly status: CapabilityDetail["status"];
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
							operation.kind !== "sync" ||
							operation.capability_id !== input.capability_id ||
							operation.state !== "syncing"
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Provider sync completion has no matching claim",
							});
						yield* transaction
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
							});
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
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						return sequence;
					}),
				)
				.pipe(
					Effect.tap((sequence) => notifier.Publish(sequence)),
					Effect.asVoid,
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Provider sync completion could not be persisted",
								}),
					),
				);
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (existing) {
							if (
								existing.kind !== "invoke" ||
								existing.capability_id !== input.capability_id ||
								existing.request_fingerprint !== input.request_fingerprint ||
								existing.tool_name !== input.tool_name ||
								existing.approval_id !== (input.approval_id ?? null) ||
								existing.approval_fingerprint !==
									(input.approval_fingerprint ?? null)
							)
								return yield* new CapabilityRepositoryError({
									code: "conflict",
									message:
										"Capability invocation id was reused with different intent",
								});
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
					Effect.mapError(
						() =>
							new CapabilityRepositoryError({
								code: "invariant",
								message: "Capability invocation request could not be persisted",
							}),
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (!operation || operation.kind !== "invoke")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation approval has no matching request",
							});
						if (
							operation.approval_id !== (input.approval_id ?? null) ||
							operation.approval_fingerprint !== (input.approval_fingerprint ?? null)
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message:
									"Capability invocation approval does not match its request",
							});
						if (operation.state === "completed")
							return { result: "completed" as const };
						const state: "approved" | "denied" = input.approved ? "approved" : "denied";
						if (operation.state === state) return { result: "duplicate" as const };
						if (operation.state !== "awaiting_approval")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation approval was already decided",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({ approval_decision: state, state, updated_at: now })
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						const sequence = yield* Append(transaction, {
							capability_id: operation.capability_id,
							invocation_status: input.approved ? "approved" : "denied",
							operation: "invoked",
							operation_id: `${input.operation_id}:decision`,
							status: input.status,
							...(operation.tool_name === null
								? {}
								: { tool_name: operation.tool_name }),
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
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message:
										"Capability invocation approval could not be persisted",
								}),
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
									new CapabilityRepositoryError({
										code: "not_found",
										message: "Capability invocation approval was not found",
									}),
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
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability invocation approval could not be read",
								}),
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation claim has no matching request",
							});
						if (operation.state === "completed") return "completed" as const;
						if (operation.state === "denied") return "denied" as const;
						if (operation.state === "executing") return "executing" as const;
						if (operation.state !== "approved")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation is not approved",
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
									message: "Capability invocation claim could not be persisted",
								}),
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (
							!operation ||
							operation.kind !== "invoke" ||
							operation.capability_id !== input.capability_id ||
							operation.tool_name !== input.tool_name
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability completion has no matching invocation",
							});
						if (
							operation.state === "completed" &&
							operation.artifact_id !== input.artifact_id
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation completion changed its artifact",
							});
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
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation is not executing",
							});
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
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
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
					Effect.map(({ metadata }) => metadata),
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
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							)
							.limit(1);
						if (
							!operation ||
							operation.kind !== "invoke" ||
							operation.capability_id !== input.capability_id ||
							operation.tool_name !== input.tool_name
						)
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability failure has no matching invocation",
							});
						const metadata_result = {
							approval_required: input.approval_required,
							capability_id: input.capability_id,
							invocation_id: input.operation_id,
							status: "failed",
							tool_name: input.tool_name,
						} satisfies CapabilityInvocationMetadata;
						if (operation.state === "failed") return { metadata: metadata_result };
						if (operation.state !== "executing")
							return yield* new CapabilityRepositoryError({
								code: "conflict",
								message: "Capability invocation is not executing",
							});
						const now = yield* metadata.Now;
						yield* transaction
							.update(MarketplaceCapabilityOperations)
							.set({
								failure_code: "tool_call_failed",
								state: "failed",
								updated_at: now,
							})
							.where(
								eq(
									MarketplaceCapabilityOperations.operation_id,
									input.operation_id,
								),
							);
						const sequence = yield* Append(transaction, {
							capability_id: input.capability_id,
							invocation_status: "failed",
							operation: "invoked",
							operation_id: `${input.operation_id}:failed`,
							status: input.status,
							tool_name: input.tool_name,
						});
						return { metadata: metadata_result, sequence };
					}),
				)
				.pipe(
					Effect.tap(({ sequence }) =>
						sequence === undefined ? Effect.void : notifier.Publish(sequence),
					),
					Effect.map(({ metadata }) => metadata),
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
									new CapabilityRepositoryError({
										code: "not_found",
										message: "Capability invocation artifact was not found",
									}),
								)
							: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
									artifact.result_json,
								).pipe(
									Effect.mapError(
										() =>
											new CapabilityRepositoryError({
												code: "invariant",
												message:
													"Capability invocation artifact is corrupt",
											}),
									),
								),
					),
					Effect.mapError((error) =>
						error instanceof CapabilityRepositoryError
							? error
							: new CapabilityRepositoryError({
									code: "invariant",
									message: "Capability invocation artifact could not be read",
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
			RecordDriftResolution,
			DecideDriftOverwrite,
			ReadDriftApproval,
			ClaimDriftOverwrite,
			CompleteDriftResolution,
			RecordProviderSync,
			ClaimProviderSync,
			CompleteProviderSync,
			DecideConnect,
			ClaimConnect,
			ReadApprovedConnect,
			RecordSessionAction,
			ClaimSessionAction,
			CompleteSessionAction,
			RecordInvocation,
			ReadInvocationApproval,
			DecideInvocation,
			ClaimInvocation,
			CompleteInvocation,
			FailInvocation,
			ReadInvocationArtifact,
			RecordOAuthOperation,
			ClaimOAuthOperation,
			ReadOAuthBeginResult,
			CompleteOAuthOperation,
			ReadDetail,
			ReadSummaries,
			SetMirror: (input: {
				readonly capability_id: string;
				readonly state: ProviderSyncState;
			}) => SetMirror(input).pipe(Effect.orDie),
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
			}) => Transition(input).pipe(Effect.orDie),
		};
	}).pipe(Effect.orDie),
);
