import { and, asc, eq, or, sql } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";

import {
	DecodeRoutineInstallDecisionRequest,
	DecodeRoutineInstallRequest,
	MarketplaceArtifactIdentity,
	RoutineInstallApproval,
	RoutineInstallDecisionRequest,
	RoutineInstallPreview,
	RoutineInstallation,
	RoutineListQuery,
	RoutineListResult,
	RoutineReadQuery,
	RoutineReadResult,
	type RoutineInstallApproval as RoutineInstallApprovalValue,
	type RoutineInstallation as RoutineInstallationValue,
	type RoutineListResult as RoutineListResultValue,
	type RoutineReadResult as RoutineReadResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { RoutineInstallApprovals, RoutineInstallationHistory } from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const text_encoder = new TextEncoder();

type ApprovalRow = typeof RoutineInstallApprovals.$inferSelect;
type InstallationRow = typeof RoutineInstallationHistory.$inferSelect;

export class RoutineInstallConflict extends Data.TaggedError("RoutineInstallConflict")<{
	readonly reason:
		| "approval_conflict"
		| "decision_conflict"
		| "install_conflict"
		| "slot_conflict"
		| "terminal_state";
}> {}

export class RoutineInstallUnavailable extends Data.TaggedError("RoutineInstallUnavailable")<{
	readonly reason: "missing";
}> {}

export class RoutineInstallInvariant extends Data.TaggedError("RoutineInstallInvariant")<{
	readonly message: string;
}> {}

export class RoutineInstallPersistenceFailure extends Data.TaggedError(
	"RoutineInstallPersistenceFailure",
)<{
	readonly reason: "unavailable";
}> {}

export type RoutineInstallRepositoryError =
	| RoutineInstallConflict
	| RoutineInstallInvariant
	| RoutineInstallPersistenceFailure
	| RoutineInstallUnavailable;

/** Owns durable source-safe Routine approval and installation settlement. */
export class RoutineInstallRepository extends Context.Service<
	RoutineInstallRepository,
	{
		readonly Decide: (
			input: unknown,
		) => Effect.Effect<RoutineInstallApprovalValue, RoutineInstallRepositoryError>;
		readonly Get: (
			input: unknown,
		) => Effect.Effect<RoutineReadResultValue, RoutineInstallRepositoryError>;
		readonly Install: (
			input: unknown,
		) => Effect.Effect<RoutineInstallationValue, RoutineInstallRepositoryError>;
		readonly List: (
			input: unknown,
		) => Effect.Effect<RoutineListResultValue, RoutineInstallRepositoryError>;
		readonly Preview: (
			input: unknown,
		) => Effect.Effect<RoutineInstallApprovalValue, RoutineInstallRepositoryError>;
	}
>()("Artisan/RoutineInstallRepository") {}

function canonical_json(value: unknown): string {
	if (Array.isArray(value)) {
		return `[${value.map(canonical_json).join(",")}]`;
	}

	if (typeof value === "object" && value !== null) {
		const record = value as Readonly<Record<string, unknown>>;

		return `{${Object.keys(record)
			.toSorted()
			.map((key) => `${JSON.stringify(key)}:${canonical_json(record[key])}`)
			.join(",")}}`;
	}

	return JSON.stringify(value);
}

function scope_slot(scope: RoutineInstallationValue["routine"]["scope"]): string {
	return scope.kind === "global"
		? "global"
		: `${scope.kind}:${scope.kind === "workspace" ? scope.workspace_id : scope.project_id}`;
}

function invariant(message: string) {
	return new RoutineInstallInvariant({ message });
}

function normalize_error(error: unknown): RoutineInstallRepositoryError {
	if (
		error instanceof RoutineInstallConflict ||
		error instanceof RoutineInstallInvariant ||
		error instanceof RoutineInstallPersistenceFailure ||
		error instanceof RoutineInstallUnavailable
	) {
		return error;
	}

	return new RoutineInstallPersistenceFailure({ reason: "unavailable" });
}

function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string, message: string) {
	return Effect.try({
		try: () => JSON.parse(value) as unknown,
		catch: () => invariant(message),
	}).pipe(
		Effect.flatMap((parsed) =>
			Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(parsed).pipe(
				Effect.mapError(() => invariant(message)),
			),
		),
	);
}

/** Uses the persisted candidate rather than any caller-provided Routine projection. */
function installation_from_preview(
	preview: typeof RoutineInstallPreview.Type,
	installation_id: string,
	updated_at: string,
) {
	const candidate = preview.candidate;

	return {
		installation_id,
		routine: {
			...candidate,
			lifecycle: "enabled" as const,
			sync: candidate.compatibility.map((engine) => ({
				drift: "none" as const,
				engine,
				identity: candidate.summary.identity,
				status: "runtime_only" as const,
				updated_at,
			})),
			updated_at,
		},
	};
}

export const RoutineInstallRepositoryLive = Layer.effect(
	RoutineInstallRepository,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const Hash = (value: string) =>
			crypto.digest("SHA-256", text_encoder.encode(value)).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() =>
					invariant("Routine installation fingerprint could not be computed"),
				),
			);

		const DecodeApproval = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const preview = yield* DecodeStoredJson(
					RoutineInstallPreview,
					row.preview_json,
					`Routine approval ${row.approval_id} has invalid preview data`,
				);
				const preview_fingerprint = yield* Hash(canonical_json(preview));

				if (preview_fingerprint !== row.preview_fingerprint) {
					return yield* invariant(`Routine approval ${row.approval_id} is corrupt`);
				}

				const approval = {
					approval_id: row.approval_id,
					decision: row.decision,
					preview,
					preview_operation_id: row.preview_operation_id,
					updated_at: row.updated_at,
				};

				return yield* Schema.decodeUnknownEffect(RoutineInstallApproval, {
					onExcessProperty: "error",
				})(approval).pipe(
					Effect.mapError(() =>
						invariant(`Routine approval ${row.approval_id} is corrupt`),
					),
				);
			});

		const DecodeTerminalDecision = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const decision_id = row.decision_id;
				const decision_operation_id = row.decision_operation_id;
				const decision_request_source = row.decision_request_json;
				const decision_snapshot_source = row.decision_snapshot_json;
				const decided_at = row.decided_at;
				const terminal_decision =
					row.decision === "applied"
						? "approved"
						: row.decision === "approved" || row.decision === "denied"
							? row.decision
							: undefined;

				if (
					terminal_decision === undefined ||
					decision_id === null ||
					decision_operation_id === null ||
					decision_request_source === null ||
					decision_snapshot_source === null ||
					decided_at === null
				) {
					return yield* invariant(`Routine approval ${row.approval_id} is corrupt`);
				}

				const approval = yield* DecodeApproval(row);
				const decision_request = yield* DecodeStoredJson(
					RoutineInstallDecisionRequest,
					decision_request_source,
					`Routine approval ${row.approval_id} has invalid decision request data`,
				);
				const decision_snapshot = yield* DecodeStoredJson(
					RoutineInstallApproval,
					decision_snapshot_source,
					`Routine approval ${row.approval_id} has invalid decision snapshot data`,
				);
				const pending_approval = {
					...approval,
					decision: "pending" as const,
					updated_at: row.created_at,
				};
				const expected_decision_request = {
					approval: pending_approval,
					approval_id: row.approval_id,
					decision: terminal_decision,
					decision_id,
					operation_id: decision_operation_id,
					preview_operation_id: row.preview_operation_id,
				};
				const expected_decision_snapshot = {
					...approval,
					decision: terminal_decision,
					updated_at: decided_at,
				};
				const decision_timestamp_is_valid =
					decision_snapshot.updated_at === decided_at &&
					decided_at >= row.created_at &&
					decided_at <= row.updated_at &&
					(row.decision === "applied" || decided_at === row.updated_at);
				const decision_request_json = canonical_json(decision_request);
				const decision_snapshot_json = canonical_json(decision_snapshot);

				if (
					!decision_timestamp_is_valid ||
					decision_request_source !== decision_request_json ||
					decision_snapshot_source !== decision_snapshot_json ||
					decision_request_json !== canonical_json(expected_decision_request) ||
					decision_snapshot_json !== canonical_json(expected_decision_snapshot)
				) {
					return yield* invariant(`Routine approval ${row.approval_id} is corrupt`);
				}

				return { approval, decision_snapshot };
			});

		const DecodeInstallation = (row: InstallationRow, approval_row: ApprovalRow) =>
			Effect.gen(function* () {
				const stored = yield* Effect.try({
					try: () => ({
						installation_id: row.installation_id,
						routine: JSON.parse(row.routine_json) as unknown,
					}),
					catch: () =>
						invariant(`Routine installation ${row.installation_id} is corrupt`),
				});
				const installation = yield* Schema.decodeUnknownEffect(RoutineInstallation, {
					onExcessProperty: "error",
				})(stored).pipe(
					Effect.mapError(() =>
						invariant(`Routine installation ${row.installation_id} is corrupt`),
					),
				);
				const terminal = yield* DecodeTerminalDecision(approval_row);
				const approval = terminal.approval;
				const decision_snapshot = terminal.decision_snapshot;
				const rollback_identity = yield* DecodeStoredJson(
					MarketplaceArtifactIdentity,
					row.rollback_identity_json,
					`Routine installation ${row.installation_id} has invalid rollback data`,
				);
				const expected = yield* Schema.decodeUnknownEffect(RoutineInstallation, {
					onExcessProperty: "error",
				})(
					installation_from_preview(
						decision_snapshot.preview,
						row.installation_id,
						row.installed_at,
					),
				).pipe(
					Effect.mapError(() =>
						invariant(`Routine installation ${row.installation_id} is corrupt`),
					),
				);
				const preview = decision_snapshot.preview;

				if (
					approval_row.approval_id !== row.approval_id ||
					approval.decision !== "applied" ||
					approval.updated_at !== row.installed_at ||
					row.routine_id !== expected.routine.summary.routine_id ||
					row.scope_slot !== scope_slot(expected.routine.scope) ||
					row.installation_id !== preview.rollback.installation_id ||
					row.rollback_id !== preview.rollback.rollback_id ||
					row.rollback_plan_fingerprint !== preview.rollback.plan_fingerprint ||
					row.rollback_plan_version !== preview.rollback.plan_version ||
					canonical_json(rollback_identity) !==
						canonical_json(preview.rollback.identity) ||
					canonical_json(installation) !== canonical_json(expected)
				) {
					return yield* invariant(
						`Routine installation ${row.installation_id} is corrupt`,
					);
				}

				return { decision_snapshot, installation };
			});

		const Preview = (input: unknown) =>
			Schema.decodeUnknownEffect(RoutineInstallPreview, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => new RoutineInstallConflict({ reason: "approval_conflict" })),
				Effect.flatMap((preview) =>
					Effect.gen(function* () {
						const preview_json = canonical_json(preview);
						const preview_fingerprint = yield* Hash(preview_json);
						const approval_id = yield* metadata.MakeId("approval");

						return yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const [existing] = yield* transaction
										.select()
										.from(RoutineInstallApprovals)
										.where(
											eq(
												RoutineInstallApprovals.preview_operation_id,
												preview.preview_operation_id,
											),
										)
										.limit(1);

									if (existing) {
										if (existing.preview_fingerprint !== preview_fingerprint) {
											return yield* new RoutineInstallConflict({
												reason: "approval_conflict",
											});
										}

										return yield* DecodeApproval(existing);
									}

									const now = yield* metadata.Now;
									const [inserted] = yield* transaction
										.insert(RoutineInstallApprovals)
										.values({
											approval_id,
											created_at: now,
											preview_fingerprint,
											preview_json,
											preview_operation_id: preview.preview_operation_id,
											updated_at: now,
										})
										.onConflictDoNothing()
										.returning();

									if (inserted) {
										return yield* DecodeApproval(inserted);
									}

									const [concurrent] = yield* transaction
										.select()
										.from(RoutineInstallApprovals)
										.where(
											eq(
												RoutineInstallApprovals.preview_operation_id,
												preview.preview_operation_id,
											),
										)
										.limit(1);

									if (!concurrent) {
										return yield* invariant(
											"Routine approval insert race was lost",
										);
									}

									if (concurrent.preview_fingerprint !== preview_fingerprint) {
										return yield* new RoutineInstallConflict({
											reason: "approval_conflict",
										});
									}

									return yield* DecodeApproval(concurrent);
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const Decide = (input: unknown) =>
			DecodeRoutineInstallDecisionRequest(input).pipe(
				Effect.mapError(() => new RoutineInstallConflict({ reason: "decision_conflict" })),
				Effect.flatMap((request) =>
					Effect.gen(function* () {
						const request_json = canonical_json(request);

						return yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const [row] = yield* transaction
										.select()
										.from(RoutineInstallApprovals)
										.where(
											eq(
												RoutineInstallApprovals.approval_id,
												request.approval_id,
											),
										)
										.limit(1);

									if (!row) {
										return yield* new RoutineInstallUnavailable({
											reason: "missing",
										});
									}

									if (row.decision !== "pending") {
										const terminal = yield* DecodeTerminalDecision(row);

										if (
											row.decision_id !== request.decision_id ||
											row.decision_operation_id !== request.operation_id ||
											row.decision_request_json !== request_json
										) {
											return yield* new RoutineInstallConflict({
												reason: "decision_conflict",
											});
										}

										return terminal.decision_snapshot;
									}

									const pending = yield* DecodeApproval(row);

									if (
										canonical_json(pending) !== canonical_json(request.approval)
									) {
										return yield* new RoutineInstallConflict({
											reason: "decision_conflict",
										});
									}

									const now = yield* metadata.Now;
									const decision_snapshot = {
										...pending,
										decision: request.decision,
										updated_at: now,
									};
									const decision_snapshot_json =
										canonical_json(decision_snapshot);
									const [updated] = yield* transaction
										.update(RoutineInstallApprovals)
										.set({
											decision: request.decision,
											decision_id: request.decision_id,
											decision_operation_id: request.operation_id,
											decision_request_json: request_json,
											decision_snapshot_json,
											decided_at: now,
											updated_at: now,
										})
										.where(
											and(
												eq(
													RoutineInstallApprovals.approval_id,
													request.approval_id,
												),
												eq(RoutineInstallApprovals.decision, "pending"),
											),
										)
										.returning();

									if (!updated) {
										return yield* new RoutineInstallConflict({
											reason: "decision_conflict",
										});
									}

									const terminal = yield* DecodeTerminalDecision(updated);

									return terminal.decision_snapshot;
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const Install = (input: unknown) =>
			DecodeRoutineInstallRequest(input).pipe(
				Effect.mapError(() => new RoutineInstallConflict({ reason: "install_conflict" })),
				Effect.flatMap((request) =>
					Effect.gen(function* () {
						return yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const [approval_row] = yield* transaction
										.select()
										.from(RoutineInstallApprovals)
										.where(
											eq(
												RoutineInstallApprovals.approval_id,
												request.approval_id,
											),
										)
										.limit(1);

									if (!approval_row) {
										return yield* new RoutineInstallUnavailable({
											reason: "missing",
										});
									}

									const [existing] = yield* transaction
										.select()
										.from(RoutineInstallationHistory)
										.where(
											or(
												eq(
													RoutineInstallationHistory.approval_id,
													request.approval_id,
												),
												eq(
													RoutineInstallationHistory.installation_id,
													request.installation_id,
												),
												eq(
													RoutineInstallationHistory.install_operation_id,
													request.operation_id,
												),
											),
										)
										.limit(1);

									if (existing) {
										const decoded = yield* DecodeInstallation(
											existing,
											approval_row,
										);

										if (
											existing.approval_id !== request.approval_id ||
											existing.installation_id !== request.installation_id ||
											existing.install_operation_id !==
												request.operation_id ||
											existing.scope_slot !== scope_slot(request.scope) ||
											request.preview_operation_id !==
												approval_row.preview_operation_id ||
											canonical_json(request.approval) !==
												canonical_json(decoded.decision_snapshot)
										) {
											return yield* new RoutineInstallConflict({
												reason: "install_conflict",
											});
										}

										return decoded.installation;
									}

									if (approval_row.decision === "denied") {
										return yield* new RoutineInstallConflict({
											reason: "terminal_state",
										});
									}

									if (approval_row.decision !== "approved") {
										return yield* new RoutineInstallConflict({
											reason: "install_conflict",
										});
									}

									const terminal = yield* DecodeTerminalDecision(approval_row);
									const approved = terminal.decision_snapshot;

									if (
										canonical_json(approved) !==
										canonical_json(request.approval)
									) {
										return yield* new RoutineInstallConflict({
											reason: "install_conflict",
										});
									}

									const preview = approved.preview;
									const now = yield* metadata.Now;
									const installation = yield* Schema.decodeUnknownEffect(
										RoutineInstallation,
										{
											onExcessProperty: "error",
										},
									)(
										installation_from_preview(
											preview,
											request.installation_id,
											now,
										),
									).pipe(
										Effect.mapError(() =>
											invariant("Derived Routine installation is invalid"),
										),
									);
									const slot = scope_slot(request.scope);
									const [active] = yield* transaction
										.select({
											installation_id:
												RoutineInstallationHistory.installation_id,
										})
										.from(RoutineInstallationHistory)
										.where(
											and(
												eq(
													RoutineInstallationHistory.routine_id,
													installation.routine.summary.routine_id,
												),
												eq(RoutineInstallationHistory.scope_slot, slot),
												eq(RoutineInstallationHistory.is_active, true),
											),
										)
										.limit(1);

									if (active) {
										return yield* new RoutineInstallConflict({
											reason: "slot_conflict",
										});
									}

									const [version_row] = yield* transaction
										.select({
											install_version: sql<number>`coalesce(max(${RoutineInstallationHistory.install_version}), 0)`,
										})
										.from(RoutineInstallationHistory)
										.where(
											and(
												eq(
													RoutineInstallationHistory.routine_id,
													installation.routine.summary.routine_id,
												),
												eq(RoutineInstallationHistory.scope_slot, slot),
											),
										);

									const install_version = (version_row?.install_version ?? 0) + 1;
									const [inserted] = yield* transaction
										.insert(RoutineInstallationHistory)
										.values({
											approval_id: request.approval_id,
											install_operation_id: request.operation_id,
											install_version,
											installation_id: request.installation_id,
											installed_at: now,
											is_active: true,
											rollback_id: preview.rollback.rollback_id,
											rollback_identity_json: canonical_json(
												preview.rollback.identity,
											),
											rollback_plan_fingerprint:
												preview.rollback.plan_fingerprint,
											rollback_plan_version: preview.rollback.plan_version,
											routine_id: installation.routine.summary.routine_id,
											routine_json: canonical_json(installation.routine),
											scope_slot: slot,
										})
										.returning();

									if (!inserted) {
										return yield* invariant(
											"Routine installation insert was not returned",
										);
									}

									const [applied] = yield* transaction
										.update(RoutineInstallApprovals)
										.set({ decision: "applied", updated_at: now })
										.where(
											and(
												eq(
													RoutineInstallApprovals.approval_id,
													request.approval_id,
												),
												eq(RoutineInstallApprovals.decision, "approved"),
											),
										)
										.returning();

									if (!applied) {
										return yield* invariant(
											"Routine approval could not be settled",
										);
									}

									const decoded = yield* DecodeInstallation(inserted, applied);

									return decoded.installation;
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);

		const List = (input: unknown) =>
			Schema.decodeUnknownEffect(RoutineListQuery, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => new RoutineInstallUnavailable({ reason: "missing" })),
				Effect.flatMap((query) =>
					database.client
						.select({
							approval: RoutineInstallApprovals,
							installation: RoutineInstallationHistory,
						})
						.from(RoutineInstallationHistory)
						.leftJoin(
							RoutineInstallApprovals,
							eq(
								RoutineInstallApprovals.approval_id,
								RoutineInstallationHistory.approval_id,
							),
						)
						.where(
							and(
								eq(
									RoutineInstallationHistory.scope_slot,
									scope_slot(query.context.scope),
								),
								eq(RoutineInstallationHistory.is_active, true),
							),
						)
						.orderBy(asc(RoutineInstallationHistory.installation_id))
						.pipe(
							Effect.flatMap((rows) =>
								Effect.forEach(rows, ({ approval, installation }) =>
									approval === null
										? Effect.fail(
												invariant(
													`Routine installation ${installation.installation_id} is corrupt`,
												),
											)
										: DecodeInstallation(installation, approval).pipe(
												Effect.map((decoded) => decoded.installation),
											),
								).pipe(
									Effect.flatMap((installations) =>
										Schema.decodeUnknownEffect(RoutineListResult, {
											onExcessProperty: "error",
										})({
											query,
											routines: installations
												.filter(
													(installation) =>
														installation.routine.lifecycle ===
															"enabled" &&
														installation.routine.compatibility.includes(
															query.context.engine,
														),
												)
												.map((installation) => ({
													engine: query.context.engine,
													installation,
													state: "eligible" as const,
												})),
										}).pipe(
											Effect.mapError(() =>
												invariant("Routine list is corrupt"),
											),
										),
									),
								),
							),
						),
				),
				Effect.mapError(normalize_error),
			);

		const Get = (input: unknown) =>
			Schema.decodeUnknownEffect(RoutineReadQuery, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(() => new RoutineInstallUnavailable({ reason: "missing" })),
				Effect.flatMap((query) =>
					database.client
						.select({
							approval: RoutineInstallApprovals,
							installation: RoutineInstallationHistory,
						})
						.from(RoutineInstallationHistory)
						.leftJoin(
							RoutineInstallApprovals,
							eq(
								RoutineInstallApprovals.approval_id,
								RoutineInstallationHistory.approval_id,
							),
						)
						.where(
							and(
								eq(
									RoutineInstallationHistory.installation_id,
									query.routine.installation_id,
								),
								eq(RoutineInstallationHistory.is_active, true),
							),
						)
						.limit(1)
						.pipe(
							Effect.flatMap(([row]) => {
								if (row === undefined) {
									return Effect.succeed(undefined);
								}

								if (row.approval === null) {
									return Effect.fail(
										invariant(
											`Routine installation ${row.installation.installation_id} is corrupt`,
										),
									);
								}

								return DecodeInstallation(row.installation, row.approval).pipe(
									Effect.map((decoded) => decoded.installation),
								);
							}),
							Effect.flatMap((installation) =>
								Schema.decodeUnknownEffect(RoutineReadResult, {
									onExcessProperty: "error",
								})({
									query,
									...(installation !== undefined &&
									installation.routine.lifecycle === "enabled" &&
									installation.routine.compatibility.includes(
										query.context.engine,
									) &&
									installation.routine.summary.routine_id ===
										query.routine.routine_id &&
									scope_slot(installation.routine.scope) ===
										scope_slot(query.routine.scope) &&
									canonical_json(installation.routine.summary.identity) ===
										canonical_json(query.routine.identity)
										? {
												routine: {
													engine: query.context.engine,
													installation,
													state: "eligible" as const,
												},
											}
										: {}),
								}).pipe(
									Effect.mapError(() => invariant("Routine read is corrupt")),
								),
							),
						),
				),
				Effect.mapError(normalize_error),
			);

		return { Decide, Get, Install, List, Preview };
	}),
);
