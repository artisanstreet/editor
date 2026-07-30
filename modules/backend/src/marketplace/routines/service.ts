import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	MarketplaceBrowseQuery,
	RoutineDetail,
	RoutineInstallPreview,
	RoutineInstallRequest,
	RoutineInvocationMetadata,
	RoutineInvocationRequest,
	RoutineSummary,
	NpxSkillsDiscoveryRequest,
	NpxSkillsDiscoveryResult,
	NpxSkillsImportRequest,
} from "@artisan/protocol";

import {
	RoutineInstaller,
	RoutineInstallerError,
	RoutineInstallReceiptSchema,
	RoutineInspectorError,
	RoutineInspectionSchema,
	type RoutineInspection,
	RoutineMirrorDriftResult,
	RoutineMirrorRegistry,
	RoutineMirrorSyncResult,
	NpxSkillsAdapter,
	RoutineSourceInspector,
} from "./adapters";
import { RoutineRepository, RoutineRepositoryError } from "./repository";
import { RuntimeMetadata } from "../../runtime/metadata";

export class RoutineServiceError extends Data.TaggedError("RoutineServiceError")<{
	readonly code: "approval_required" | "disabled" | "ineligible" | "preview_changed" | "rejected";
	readonly message: string;
}> {}

export interface RoutineInstallDecision {
	readonly approved: boolean;
	readonly operation_id: string;
	readonly request_fingerprint: string;
}

const Fingerprint = (value: unknown) =>
	createHash("sha256").update(JSON.stringify(value)).digest("hex");

const Matches = (routine: RoutineSummary, query: MarketplaceBrowseQuery) =>
	(query.category === undefined || query.category === "routine") &&
	(query.enabled === undefined || query.enabled === routine.enabled) &&
	(query.status === undefined || query.status === routine.status) &&
	(query.scope === undefined || JSON.stringify(query.scope) === JSON.stringify(routine.scope)) &&
	(query.text === undefined ||
		routine.display_name.toLocaleLowerCase().includes(query.text.toLocaleLowerCase()) ||
		routine.description.toLocaleLowerCase().includes(query.text.toLocaleLowerCase()));

/** Approval-gated canonical routine coordinator. Its layer has no source, provider, or installer side effects. */
export class RoutineService extends Context.Service<
	RoutineService,
	{
		readonly Preview: (input: {
			readonly scope: RoutineDetail["scope"];
			readonly source: RoutineDetail["source"];
		}) => Effect.Effect<RoutineInstallPreview, RoutineInspectorError>;
		readonly Browse: (
			query: MarketplaceBrowseQuery,
		) => Effect.Effect<ReadonlyArray<RoutineSummary>, RoutineRepositoryError>;
		readonly Detail: (
			routine_id: string,
		) => Effect.Effect<RoutineDetail, RoutineRepositoryError>;
		readonly RequestInstall: (
			input: RoutineInstallRequest &
				Pick<RoutineInstallDecision, "operation_id" | "request_fingerprint">,
		) => Effect.Effect<
			RoutineInstallPreview,
			RoutineInspectorError | RoutineRepositoryError | RoutineServiceError
		>;
		readonly DecideInstall: (
			input: RoutineInstallRequest & RoutineInstallDecision,
		) => Effect.Effect<
			RoutineDetail,
			| RoutineInstallerError
			| RoutineInspectorError
			| RoutineRepositoryError
			| RoutineServiceError
		>;
		readonly Install: (
			input: RoutineInstallRequest & RoutineInstallDecision,
		) => Effect.Effect<
			RoutineDetail,
			| RoutineInstallerError
			| RoutineInspectorError
			| RoutineRepositoryError
			| RoutineServiceError
		>;
		readonly Enable: (input: {
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<number, RoutineRepositoryError | RoutineServiceError>;
		readonly Disable: (input: {
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<number, RoutineRepositoryError | RoutineServiceError>;
		readonly Remove: (input: {
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<number, RoutineRepositoryError | RoutineServiceError>;
		readonly Rollback: (input: {
			readonly operation_id: string;
			readonly rollback_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<
			number,
			RoutineInstallerError | RoutineRepositoryError | RoutineServiceError
		>;
		readonly RecoverRollbacks: Effect.Effect<
			ReadonlyArray<number>,
			RoutineInstallerError | RoutineRepositoryError
		>;
		readonly Invoke: (
			input: RoutineInvocationRequest & {
				readonly engine_id: string;
				readonly operation_id: string;
			},
		) => Effect.Effect<RoutineInvocationMetadata, RoutineRepositoryError | RoutineServiceError>;
		readonly Sync: (input: {
			readonly engine_id: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<
			number,
			RoutineInstallerError | RoutineRepositoryError | RoutineServiceError
		>;
		readonly ResolveDrift: (input: {
			readonly action: "ignore" | "import";
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<
			number,
			RoutineInstallerError | RoutineRepositoryError | RoutineServiceError
		>;
		/** Executes only an overwrite whose exact durable approval was resolved by the protocol edge. */
		readonly ExecuteApprovedDriftOverwrite: (input: {
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) => Effect.Effect<
			number,
			RoutineInstallerError | RoutineRepositoryError | RoutineServiceError
		>;
		/** Discovery output is a transient source candidate; canonical storage happens through the approval flow. */
		readonly DiscoverNpxSkills: (
			input: NpxSkillsDiscoveryRequest,
		) => Effect.Effect<NpxSkillsDiscoveryResult, RoutineInspectorError>;
		readonly PreviewNpxImport: (
			input: NpxSkillsImportRequest,
		) => Effect.Effect<RoutineInstallPreview, RoutineInspectorError | RoutineServiceError>;
	}
>()("Artisan/Marketplace/RoutineService") {}

export const RoutineServiceLive = Layer.effect(
	RoutineService,
	Effect.gen(function* () {
		const inspector = yield* RoutineSourceInspector;
		const installer = yield* RoutineInstaller;
		const mirrors = yield* RoutineMirrorRegistry;
		const npx = yield* NpxSkillsAdapter;
		const repository = yield* RoutineRepository;
		const metadata = yield* RuntimeMetadata;
		const InspectSource = (input: {
			readonly scope: RoutineDetail["scope"];
			readonly source: RoutineDetail["source"];
		}) =>
			inspector.Inspect(input).pipe(
				Effect.flatMap(Schema.decodeUnknownEffect(RoutineInspectionSchema)),
				Effect.map((decoded) => {
					const { author, ...inspection } = decoded;
					return {
						...inspection,
						...(author === undefined ? {} : { author }),
					} satisfies RoutineInspection;
				}),
				Effect.mapError((error) =>
					error instanceof RoutineInspectorError
						? error
						: new RoutineInspectorError({ code: "invalid_source" }),
				),
			);

		const Preview = (input: {
			readonly scope: RoutineDetail["scope"];
			readonly source: RoutineDetail["source"];
		}) =>
			InspectSource(input).pipe(
				Effect.map((inspection) => ({
					candidate_id: inspection.candidate_id,
					candidate_name: inspection.display_name,
					compatibility: [...inspection.compatibility],
					files: [...inspection.files],
					permissions: [...inspection.permissions],
					preview_fingerprint: Fingerprint({
						content_hashes: inspection.content_hashes,
						scope: input.scope,
						source: inspection.source,
					}),
					rollback_available: inspection.rollback_available,
					scope: input.scope,
					source: inspection.source,
					trust: inspection.trust,
					version: inspection.version,
				})),
			);

		const RequestInstall = (
			input: RoutineInstallRequest &
				Pick<RoutineInstallDecision, "operation_id" | "request_fingerprint">,
		) =>
			Effect.gen(function* () {
				const preview = yield* Preview({ scope: input.scope, source: input.source });
				if (preview.preview_fingerprint !== input.preview_fingerprint)
					return yield* new RoutineServiceError({
						code: "preview_changed",
						message: "Routine source changed after preview; a new approval is required",
					});
				yield* repository.RecordPendingInstall({
					approval_fingerprint: input.preview_fingerprint,
					approval_id: input.approval_id,
					operation_id: input.operation_id,
					preview_json: JSON.stringify(preview),
					request_fingerprint: input.request_fingerprint,
					routine_id: preview.candidate_id,
				});
				return preview;
			});

		const DecideInstall = (input: RoutineInstallRequest & RoutineInstallDecision) =>
			Effect.gen(function* () {
				const preview = yield* Preview({ scope: input.scope, source: input.source });
				if (preview.preview_fingerprint !== input.preview_fingerprint)
					return yield* new RoutineServiceError({
						code: "preview_changed",
						message: "Routine source changed after preview; a new approval is required",
					});
				const decision = yield* repository.DecideInstall({
					approval_fingerprint: input.preview_fingerprint,
					approval_id: input.approval_id,
					approved: input.approved,
					operation_id: input.operation_id,
				});
				if (decision === "denied")
					return yield* new RoutineServiceError({
						code: "rejected",
						message: "Routine installation was denied before any installer action",
					});
				if (decision === "installed")
					return yield* repository.ReadDetail(preview.candidate_id);
				const inspection = yield* InspectSource({
					scope: input.scope,
					source: input.source,
				});
				const inspected_fingerprint = Fingerprint({
					content_hashes: inspection.content_hashes,
					scope: input.scope,
					source: inspection.source,
				});
				if (inspected_fingerprint !== input.preview_fingerprint) {
					yield* repository.RecordInstallFailure({
						code: "conflict",
						operation_id: input.operation_id,
					});
					return yield* new RoutineServiceError({
						code: "preview_changed",
						message:
							"Routine source changed after approval; a new approval is required",
					});
				}
				const receipt = yield* installer
					.Install({ inspection, operation_id: input.operation_id, scope: input.scope })
					.pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(RoutineInstallReceiptSchema)),
						Effect.mapError((error) =>
							error instanceof RoutineInstallerError
								? error
								: new RoutineInstallerError({ code: "install_failed" }),
						),
						Effect.catch((error) =>
							repository
								.RecordInstallFailure({
									code: error.code,
									operation_id: input.operation_id,
								})
								.pipe(Effect.andThen(Effect.fail(error))),
						),
					);
				const detail: RoutineDetail = {
					...(inspection.author === undefined ? {} : { author: inspection.author }),
					compatibility: [...inspection.compatibility],
					description: inspection.description,
					display_name: inspection.display_name,
					enabled: true,
					exported_commands: [...inspection.exported_commands],
					files: [...inspection.files],
					id: inspection.candidate_id,
					instructions: inspection.instructions,
					permissions: [...inspection.permissions],
					scope: input.scope,
					source: inspection.source,
					status: "enabled",
					sync: [],
					trust: inspection.trust,
					version: inspection.version,
				};
				const commit = repository.CommitInstalled({
					artifact_refs: receipt.artifact_refs,
					detail,
					operation_id: input.operation_id,
					...(receipt.rollback_id === undefined
						? {}
						: { rollback_json: JSON.stringify({ rollback_id: receipt.rollback_id }) }),
				});
				yield* commit.pipe(
					Effect.catch((error) =>
						repository
							.RecordInstallFailure({
								code: "conflict",
								operation_id: input.operation_id,
							})
							.pipe(
								Effect.andThen(
									receipt.rollback_id === undefined
										? Effect.fail(error)
										: installer
												.Rollback({
													operation_id: `${input.operation_id}:compensate`,
													rollback_id: receipt.rollback_id,
												})
												.pipe(
													Effect.catch(() =>
														Effect.fail(
															new RoutineInstallerError({
																code: "rollback_failed",
															}),
														),
													),
													Effect.andThen(Effect.fail(error)),
												),
								),
							),
					),
				);
				return detail;
			});

		const RequireMutable = (routine_id: string) =>
			repository.ReadDetail(routine_id).pipe(
				Effect.flatMap((detail) =>
					detail.status === "removed" || detail.status === "rolled_back"
						? new RoutineServiceError({
								code: "ineligible",
								message: "Removed or rolled-back routines must be installed again",
							})
						: Effect.succeed(detail),
				),
			);

		const Enable = (input: { readonly operation_id: string; readonly routine_id: string }) =>
			Effect.gen(function* () {
				yield* RequireMutable(input.routine_id);
				return yield* repository.Transition({
					...input,
					enabled: true,
					operation: "enabled",
					status: "enabled",
				});
			});
		const Disable = (input: { readonly operation_id: string; readonly routine_id: string }) =>
			Effect.gen(function* () {
				yield* RequireMutable(input.routine_id);
				return yield* repository.Transition({
					...input,
					enabled: false,
					operation: "disabled",
					status: "disabled",
				});
			});
		const Remove = (input: { readonly operation_id: string; readonly routine_id: string }) =>
			Effect.gen(function* () {
				yield* RequireMutable(input.routine_id);
				return yield* repository.Transition({
					...input,
					enabled: false,
					operation: "removed",
					status: "removed",
				});
			});
		const Rollback = (input: {
			readonly operation_id: string;
			readonly rollback_id: string;
			readonly routine_id: string;
		}) =>
			Effect.gen(function* () {
				const claim = yield* repository.ClaimRollback(input);
				if (claim === "completed")
					return yield* repository.CommitRollback(input.operation_id);
				yield* installer.Rollback({
					operation_id: input.operation_id,
					rollback_id: input.rollback_id,
				});
				return yield* repository.CommitRollback(input.operation_id);
			});
		const RecoverRollbacks = repository.ReadRollbackRecovery.pipe(
			Effect.flatMap((claims) =>
				Effect.forEach(claims, (claim) =>
					installer
						.Rollback({
							operation_id: claim.operation_id,
							rollback_id: claim.rollback_id,
						})
						.pipe(Effect.andThen(repository.CommitRollback(claim.operation_id))),
				),
			),
		);
		const Invoke = (
			input: RoutineInvocationRequest & {
				readonly engine_id: string;
				readonly operation_id: string;
			},
		) =>
			Effect.gen(function* () {
				const detail = yield* repository.ReadDetail(input.routine_id);
				const compatibility = detail.compatibility.find(
					(entry) => entry.engine_id === input.engine_id,
				);
				if (!detail.enabled)
					return yield* new RoutineServiceError({
						code: "disabled",
						message: "Routine is disabled",
					});
				if (compatibility?.state === "unsupported")
					return yield* new RoutineServiceError({
						code: "ineligible",
						message: "Routine is unsupported by this engine",
					});
				const scope_eligible =
					detail.scope.kind === "global" ||
					(detail.scope.kind === "workspace" &&
						input.scope.kind === "workspace" &&
						detail.scope.workspace_id === input.scope.workspace_id) ||
					(detail.scope.kind === "project" &&
						input.scope.kind === "project" &&
						detail.scope.project_id === input.scope.project_id);
				if (!scope_eligible)
					return yield* new RoutineServiceError({
						code: "ineligible",
						message: "Routine is outside the current workspace or project scope",
					});
				if (
					input.command !== undefined &&
					!detail.exported_commands.some((command) => command.name === input.command)
				)
					return yield* new RoutineServiceError({
						code: "ineligible",
						message: "Routine command is not exported by the selected routine",
					});
				yield* repository.Transition({
					enabled: detail.enabled,
					operation: "invoked",
					operation_id: input.operation_id,
					routine_id: input.routine_id,
					status: detail.status,
					...(input.command === undefined ? {} : { tool_name: input.command }),
				});
				return {
					eligible: true,
					eligibility_reason:
						compatibility?.state === "native"
							? "native engine support"
							: "Artisan runtime routine path",
					invocation_id: input.operation_id,
					routine_id: detail.id,
					version: detail.version,
				};
			});
		const Sync = (input: {
			readonly engine_id: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* RequireMutable(input.routine_id);
				const claim = yield* repository.ClaimMirrorOperation({
					engine_id: input.engine_id,
					intent_fingerprint: Fingerprint(input),
					kind: "sync",
					operation_id: input.operation_id,
					routine_id: input.routine_id,
				});
				if (claim._tag === "Completed") return claim.journal_sequence;
				if (claim._tag === "InFlight")
					return yield* new RoutineRepositoryError({
						code: "conflict",
						message: "Routine provider sync is already in progress",
					});
				const updated_at = yield* metadata.Now;
				const adapter = mirrors.Find(input.engine_id);
				const compatibility = detail.compatibility.find(
					(entry) => entry.engine_id === input.engine_id,
				);
				const status =
					adapter === undefined
						? compatibility?.state === "unsupported"
							? "unsupported"
							: "runtime_only"
						: adapter.mode === "unsupported"
							? "unsupported"
							: adapter.mode === "runtime_only"
								? "runtime_only"
								: "synced";
				const result =
					status === "synced" && adapter !== undefined
						? yield* adapter
								.Sync({ operation_id: input.operation_id, routine: detail })
								.pipe(
									Effect.flatMap(
										Schema.decodeUnknownEffect(RoutineMirrorSyncResult),
									),
									Effect.mapError((error) =>
										error instanceof RoutineInstallerError
											? error
											: new RoutineInstallerError({ code: "install_failed" }),
									),
								)
						: {};
				return yield* repository.CommitMirrorOperation({
					operation_id: input.operation_id,
					state: {
						engine_id: input.engine_id,
						...(result.revision === undefined
							? {}
							: { observed_revision: result.revision }),
						status,
						updated_at,
					},
				});
			});
		const ResolveDriftInternal = (input: {
			readonly action: "ignore" | "import" | "overwrite";
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* RequireMutable(input.routine_id);
				const claim = yield* repository.ClaimMirrorOperation({
					engine_id: input.engine_id,
					intent_fingerprint: Fingerprint(input),
					kind: "drift",
					operation_id: input.operation_id,
					routine_id: input.routine_id,
				});
				if (claim._tag === "Completed") return claim.journal_sequence;
				if (claim._tag === "InFlight")
					return yield* new RoutineRepositoryError({
						code: "conflict",
						message: "Routine provider drift resolution is already in progress",
					});
				const updated_at = yield* metadata.Now;
				const adapter = mirrors.Find(input.engine_id);
				const result =
					adapter === undefined
						? { revision: input.observed_revision }
						: yield* adapter
								.ResolveDrift({
									action: input.action,
									observed_revision: input.observed_revision,
									operation_id: input.operation_id,
									routine: detail,
								})
								.pipe(
									Effect.flatMap(
										Schema.decodeUnknownEffect(RoutineMirrorDriftResult),
									),
									Effect.mapError((error) =>
										error instanceof RoutineInstallerError
											? error
											: new RoutineInstallerError({ code: "install_failed" }),
									),
								);
				if (input.action === "import") {
					if (result.imported === undefined)
						return yield* new RoutineServiceError({
							code: "ineligible",
							message: "Provider import did not return a canonical routine record",
						});
					if (result.imported.id !== detail.id)
						return yield* new RoutineServiceError({
							code: "ineligible",
							message: "Provider import returned a different routine identity",
						});
				}
				const status =
					input.action === "ignore"
						? "drift_ignored"
						: adapter === undefined
							? "runtime_only"
							: adapter.mode === "native"
								? "synced"
								: adapter.mode;
				return yield* repository.CommitMirrorOperation({
					...(result.imported === undefined ? {} : { imported: result.imported }),
					operation_id: input.operation_id,
					state: {
						engine_id: input.engine_id,
						observed_revision: result.revision ?? input.observed_revision,
						status,
						updated_at,
					},
				});
			});
		const ResolveDrift = (input: {
			readonly action: "ignore" | "import";
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) => ResolveDriftInternal(input);
		const ExecuteApprovedDriftOverwrite = (input: {
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly routine_id: string;
		}) => ResolveDriftInternal({ ...input, action: "overwrite" });
		const DiscoverNpxSkills = (input: NpxSkillsDiscoveryRequest) =>
			npx.Discover(input).pipe(
				Effect.flatMap(Schema.decodeUnknownEffect(NpxSkillsDiscoveryResult)),
				Effect.map((discovery) => ({
					...discovery,
					candidates: discovery.candidates.map((candidate) => {
						const { preview_fingerprint: _, ...reviewed } = candidate;
						return { ...reviewed, preview_fingerprint: Fingerprint(reviewed) };
					}),
				})),
				Effect.mapError((error) =>
					error instanceof RoutineInspectorError
						? error
						: new RoutineInspectorError({ code: "invalid_source" }),
				),
			);
		const PreviewNpxImport = (input: NpxSkillsImportRequest) =>
			Effect.gen(function* () {
				const discovery = yield* DiscoverNpxSkills({
					package_spec: input.package_spec,
					scope: input.scope,
				});
				const candidate = discovery.candidates.find(
					(entry) => entry.name === input.candidate_name,
				);
				if (!candidate)
					return yield* new RoutineServiceError({
						code: "ineligible",
						message: "The selected npx skills candidate is no longer available",
					});
				if (candidate.preview_fingerprint !== input.preview_fingerprint)
					return yield* new RoutineServiceError({
						code: "preview_changed",
						message:
							"The npx skills candidate changed; inspect it again before installation",
					});
				return yield* Preview({
					scope: input.scope,
					source: {
						/** npx is discovery-only; the candidate's inspected local root becomes the canonical source. */
						kind: "local",
						locator: candidate.source_locator,
					},
				});
			});

		return {
			Browse: (query) =>
				repository.ReadSummaries.pipe(
					Effect.map((routines) => routines.filter((routine) => Matches(routine, query))),
				),
			DecideInstall,
			Detail: repository.ReadDetail,
			DiscoverNpxSkills,
			Disable,
			Enable,
			ExecuteApprovedDriftOverwrite,
			Install: (input) =>
				RequestInstall(input).pipe(Effect.andThen(() => DecideInstall(input))),
			Invoke,
			Preview,
			PreviewNpxImport,
			Remove,
			RecoverRollbacks,
			RequestInstall,
			ResolveDrift,
			Rollback,
			Sync,
		};
	}),
);
