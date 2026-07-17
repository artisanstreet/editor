import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeEligibleRoutineInvocationRequest,
	DecodeRoutineInstallApproval,
	DecodeRoutineInstallCandidate,
	DecodeRoutineInstallDecisionRequest,
	DecodeRoutineInstallPreview,
	DecodeRoutineInstallPreviewRequest,
	DecodeRoutineInstallRequest,
	DecodeRoutineInstructionsResult,
	DecodeRoutineLifecycleRequest,
	DecodeRoutineListResult,
	DecodeRoutineMirrorDriftResolutionRequest,
	DecodeRoutineMirrorRetryRequest,
	DecodeRoutineReadResult,
	DecodeRoutineRollbackRequest,
	RoutineInstallRequest,
} from "@artisan/protocol";

const timestamp = "2026-07-17T08:00:00.000Z";
const later_timestamp = "2026-07-17T09:00:00.000Z";
const global_scope = { kind: "global" } as const;
const workspace_scope = { kind: "workspace", workspace_id: "workspace_1" } as const;
const project_scope = { kind: "project", project_id: "project_1" } as const;
const identity = {
	source: {
		display_name: "Artisan catalog",
		kind: "catalog",
		locator: "artisan.release-notes",
	},
	version: "1.2.3",
} as const;
const other_identity = { ...identity, version: "1.2.4" } as const;
const instructions_reference = { content_hash: "a".repeat(64) } as const;
const command = {
	command_id: "publish",
	description: "Publishes release notes.",
	label: "Publish",
} as const;
const file = {
	path: "ROUTINE.md",
	purpose: "Routine instructions.",
	write_mode: "create",
} as const;
const permission = {
	kind: "filesystem_write",
	label: "Write release notes",
	required: true,
} as const;
const trust = { level: "verified", reasons: ["Catalog review completed."] } as const;
const sync = {
	drift: "none",
	engine: "codex",
	identity,
	status: "synced",
	updated_at: timestamp,
} as const;
const routine = {
	commands: [command],
	compatibility: ["codex", "claude"],
	display_name: "Release notes",
	files: [file],
	instructions: instructions_reference,
	lifecycle: "enabled",
	permissions: [permission],
	scope: global_scope,
	summary: {
		description: "Prepares and publishes release notes.",
		display_name: "Release notes",
		identity,
		routine_id: "routine.release-notes",
	},
	sync: [sync],
	trust,
	updated_at: timestamp,
} as const;
const candidate = {
	commands: routine.commands,
	compatibility: routine.compatibility,
	display_name: routine.display_name,
	files: routine.files,
	instructions: routine.instructions,
	permissions: routine.permissions,
	scope: routine.scope,
	summary: routine.summary,
	trust: routine.trust,
} as const;
const rollback = {
	actions: ["Remove the installed Routine file."],
	available: true,
	identity,
	installation_id: "installation_1",
	plan_fingerprint: "b".repeat(64),
	plan_version: 1,
	rollback_id: "rollback_1",
	scope: global_scope,
} as const;
const preview = {
	candidate,
	preview_operation_id: "preview_1",
	rollback,
} as const;
const pending_approval = {
	approval_id: "approval_1",
	decision: "pending",
	preview,
	preview_operation_id: "preview_1",
	updated_at: timestamp,
} as const;
const approved_approval = { ...pending_approval, decision: "approved" } as const;
const applied_approval = { ...pending_approval, decision: "applied" } as const;
const installation = { installation_id: "installation_1", routine } as const;
const eligibility = { engine: "codex", installation, state: "eligible" } as const;
const list_query = { context: { engine: "codex", scope: global_scope } } as const;
const installation_reference = {
	identity,
	installation_id: "installation_1",
	routine_id: routine.summary.routine_id,
	scope: global_scope,
} as const;
const read_query = { context: list_query.context, routine: installation_reference } as const;
const invocation_context = {
	agent_id: "agent_1",
	run_id: "run_1",
	thread_id: "thread_1",
} as const;
const install_request = {
	approval: approved_approval,
	approval_id: "approval_1",
	installation_id: "installation_1",
	operation_id: "install_1",
	preview_operation_id: "preview_1",
	scope: global_scope,
} as const;
const rollback_reference = {
	identity,
	installation_id: "installation_1",
	plan_fingerprint: rollback.plan_fingerprint,
	plan_version: rollback.plan_version,
	rollback_id: rollback.rollback_id,
	scope: global_scope,
} as const;

describe("Marketplace Routine control", () => {
	it("roundtrips one complete candidate through preview, approval, decision, and install", async () => {
		await expect(Effect.runPromise(DecodeRoutineInstallCandidate(candidate))).resolves.toEqual(
			candidate,
		);
		await expect(Effect.runPromise(DecodeRoutineInstallPreview(preview))).resolves.toEqual(
			preview,
		);
		await expect(
			Effect.runPromise(DecodeRoutineInstallApproval(pending_approval)),
		).resolves.toEqual(pending_approval);
		await expect(
			Effect.runPromise(
				DecodeRoutineInstallPreviewRequest({
					candidate,
					preview_operation_id: "preview_1",
				}),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineInstallDecisionRequest({
					approval: pending_approval,
					approval_id: "approval_1",
					decision: "approved",
					decision_id: "decision_1",
					operation_id: "decide_1",
					preview_operation_id: "preview_1",
				}),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(DecodeRoutineInstallRequest(install_request)),
		).resolves.toEqual(install_request);

		const decoded = Schema.decodeUnknownSync(RoutineInstallRequest)(install_request);

		expect(Schema.encodeSync(RoutineInstallRequest)(decoded)).toEqual(install_request);
	});

	it("makes candidate understatement or replacement impossible beside the approved preview", async () => {
		for (const intrusion of [
			{ candidate: { ...candidate, summary: { ...candidate.summary, routine_id: "other" } } },
			{ files: [] },
			{ permissions: [] },
			{ rollback: { ...rollback, actions: [] } },
			{ routine_id: "other" },
			{ trust: { level: "unverified", reasons: [] } },
		]) {
			await expect(
				Effect.runPromise(
					DecodeRoutineInstallRequest({ ...install_request, ...intrusion }),
				),
			).rejects.toBeDefined();
		}

		for (const intrusion of [
			{ files: [] },
			{ permissions: [] },
			{ routine_id: "other" },
			{ trust: { level: "unverified", reasons: [] } },
		]) {
			await expect(
				Effect.runPromise(DecodeRoutineInstallPreview({ ...preview, ...intrusion })),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeRoutineInstallPreview({
					...preview,
					rollback: { ...rollback, identity: other_identity },
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineInstallApproval({
					...pending_approval,
					preview_operation_id: "preview_changed",
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects changed preview, approval, installation, and scope bindings", async () => {
		for (const changed of [
			{ ...install_request, approval_id: "approval_changed" },
			{ ...install_request, installation_id: "installation_changed" },
			{ ...install_request, preview_operation_id: "preview_changed" },
			{ ...install_request, scope: workspace_scope },
			{ ...install_request, approval: pending_approval },
		]) {
			await expect(
				Effect.runPromise(DecodeRoutineInstallRequest(changed)),
			).rejects.toBeDefined();
		}

		for (const changed of [
			{ approval_id: "approval_changed" },
			{ preview_operation_id: "preview_changed" },
			{ approval: approved_approval },
		]) {
			await expect(
				Effect.runPromise(
					DecodeRoutineInstallDecisionRequest({
						approval: pending_approval,
						approval_id: "approval_1",
						decision: "approved",
						decision_id: "decision_1",
						operation_id: "decide_1",
						preview_operation_id: "preview_1",
						...changed,
					}),
				),
			).rejects.toBeDefined();
		}
	});

	it("accepts only the four legal non-removed lifecycle transitions", async () => {
		const disabled_routine = {
			...routine,
			disabled_reason: "Disabled by the user.",
			lifecycle: "disabled",
		} as const;
		const removed_routine = { ...routine, lifecycle: "removed" } as const;
		const cases = [
			{
				installation,
				valid: new Set(["enabled_to_disabled", "enabled_to_removed"]),
			},
			{
				installation: { ...installation, routine: disabled_routine },
				valid: new Set(["disabled_to_enabled", "disabled_to_removed"]),
			},
			{
				installation: { ...installation, routine: removed_routine },
				valid: new Set<string>(),
			},
		];
		const transitions = [
			"enabled_to_disabled",
			"enabled_to_removed",
			"disabled_to_enabled",
			"disabled_to_removed",
		] as const;

		for (const lifecycle_case of cases) {
			for (const transition of transitions) {
				const decoded = Effect.runPromise(
					DecodeRoutineLifecycleRequest({
						installation: lifecycle_case.installation,
						operation_id: `lifecycle.${transition}`,
						transition,
					}),
				);

				if (lifecycle_case.valid.has(transition)) {
					await expect(decoded).resolves.toBeDefined();
				} else {
					await expect(decoded).rejects.toBeDefined();
				}
			}
		}
	});

	it("binds rollback to the exact stored plan, approval, candidate, and installation", async () => {
		const request = {
			approval: applied_approval,
			approval_id: "approval_1",
			installation,
			operation_id: "rollback_operation_1",
			plan: rollback_reference,
			preview_operation_id: "preview_1",
		} as const;

		await expect(Effect.runPromise(DecodeRoutineRollbackRequest(request))).resolves.toEqual(
			request,
		);

		for (const changed of [
			{ ...request, approval_id: "approval_changed" },
			{ ...request, preview_operation_id: "preview_changed" },
			{ ...request, plan: { ...rollback_reference, installation_id: "other" } },
			{ ...request, plan: { ...rollback_reference, plan_fingerprint: "c".repeat(64) } },
			{ ...request, plan: { ...rollback_reference, plan_version: 2 } },
			{ ...request, plan: { ...rollback_reference, rollback_id: "other" } },
			{ ...request, plan: { ...rollback_reference, scope: workspace_scope } },
			{ ...request, approval: approved_approval },
			{
				...request,
				installation: {
					...installation,
					routine: { ...routine, files: [] },
				},
			},
			{
				...request,
				installation: {
					...installation,
					routine: { ...routine, permissions: [] },
				},
			},
			{
				...request,
				installation: {
					...installation,
					routine: { ...routine, trust: { level: "unverified", reasons: [] } },
				},
			},
			{
				...request,
				installation: {
					...installation,
					routine: {
						...routine,
						summary: { ...routine.summary, routine_id: "routine.changed" },
					},
				},
			},
		]) {
			await expect(
				Effect.runPromise(DecodeRoutineRollbackRequest(changed)),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeRoutineRollbackRequest({ ...request, actions: ["Delete arbitrary file."] }),
			),
		).rejects.toBeDefined();
	});

	it("accepts only attached, current, compatible mirror rows", async () => {
		const drift_sync = {
			...sync,
			drift: "detected",
			status: "drift_detected",
			updated_at: later_timestamp,
		} as const;
		const drift_installation = {
			...installation,
			routine: { ...routine, sync: [drift_sync], updated_at: later_timestamp },
		} as const;
		const retry_sync = {
			...sync,
			last_error_code: "provider_unavailable",
			status: "sync_failed",
			updated_at: later_timestamp,
		} as const;
		const retry_installation = {
			...installation,
			routine: { ...routine, sync: [retry_sync], updated_at: later_timestamp },
		} as const;

		await expect(
			Effect.runPromise(
				DecodeRoutineMirrorDriftResolutionRequest({
					expected_sync: drift_sync,
					installation: drift_installation,
					operation_id: "resolve_1",
					resolution: "replace_mirror",
				}),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineMirrorRetryRequest({
					expected_sync: retry_sync,
					installation: retry_installation,
					operation_id: "retry_1",
				}),
			),
		).resolves.toBeDefined();

		for (const expected_sync of [
			{ ...drift_sync, engine: "claude" },
			{ ...drift_sync, updated_at: timestamp },
			{ ...drift_sync, status: "sync_failed" },
			{ ...drift_sync, identity: other_identity },
		]) {
			await expect(
				Effect.runPromise(
					DecodeRoutineMirrorDriftResolutionRequest({
						expected_sync,
						installation: drift_installation,
						operation_id: "resolve_1",
						resolution: "replace_mirror",
					}),
				),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeRoutineMirrorRetryRequest({
					expected_sync: retry_sync,
					installation: {
						...retry_installation,
						routine: { ...retry_installation.routine, compatibility: ["claude"] },
					},
					operation_id: "retry_1",
				}),
			),
		).rejects.toBeDefined();
	});

	it("supports global, workspace, and project eligibility without free attribution pairs", async () => {
		for (const scope of [global_scope, workspace_scope, project_scope]) {
			const scoped_routine = { ...routine, scope } as const;
			const scoped_installation = { ...installation, routine: scoped_routine } as const;
			const scoped_eligibility = {
				engine: "codex",
				installation: scoped_installation,
				state: "eligible",
			} as const;
			const query = { context: { engine: "codex", scope } } as const;

			await expect(
				Effect.runPromise(
					DecodeRoutineListResult({ query, routines: [scoped_eligibility] }),
				),
			).resolves.toBeDefined();
			await expect(
				Effect.runPromise(
					DecodeEligibleRoutineInvocationRequest({
						command_id: "publish",
						context: invocation_context,
						eligibility: scoped_eligibility,
						invocation_id: `invoke.${scope.kind}`,
					}),
				),
			).resolves.toBeDefined();
		}

		for (const compatibility of [[], ["codex"], ["codex", "claude"]] as const) {
			const compatible_routine = {
				...routine,
				compatibility,
				sync: compatibility.length === 0 ? [] : routine.sync,
			} as const;
			const routines =
				compatibility.length === 0
					? []
					: [
							{
								engine: "codex",
								installation: { ...installation, routine: compatible_routine },
								state: "eligible",
							},
						];

			await expect(
				Effect.runPromise(DecodeRoutineListResult({ query: list_query, routines })),
			).resolves.toBeDefined();
		}
	});

	it("keeps full instructions behind an exact hash-bound instructions result", async () => {
		const instructions_query = { eligibility } as const;
		const instructions = {
			content: "Publish the release notes.",
			content_hash: instructions_reference.content_hash,
		};

		await expect(
			Effect.runPromise(DecodeRoutineReadResult({ query: read_query, routine: eligibility })),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineInstructionsResult({ instructions, query: instructions_query }),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineInstructionsResult({
					instructions: { ...instructions, content_hash: "c".repeat(64) },
					query: instructions_query,
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineListResult({
					query: list_query,
					routines: [
						{
							...eligibility,
							installation: {
								...installation,
								routine: {
									...routine,
									instructions: { ...instructions_reference, content: "private" },
								},
							},
						},
					],
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects invocation mismatch, unverifiable attribution, and private payloads", async () => {
		const invocation = {
			command_id: "publish",
			context: invocation_context,
			eligibility,
			invocation_id: "invoke_1",
		} as const;

		await expect(
			Effect.runPromise(DecodeEligibleRoutineInvocationRequest(invocation)),
		).resolves.toEqual(invocation);

		for (const changed of [
			{ ...invocation, command_id: "missing" },
			{
				...invocation,
				eligibility: {
					...eligibility,
					installation: {
						...installation,
						routine: {
							...routine,
							disabled_reason: "Disabled by the user.",
							lifecycle: "disabled",
						},
					},
				},
			},
			{
				...invocation,
				eligibility: {
					...eligibility,
					engine: "claude",
					installation: {
						...installation,
						routine: { ...routine, compatibility: ["codex"] },
					},
				},
			},
			{ ...invocation, workspace_id: "workspace_1" },
			{ ...invocation, project_id: "project_1" },
			{ ...invocation, arguments: { private: true } },
			{ ...invocation, result: { private: true } },
		]) {
			await expect(
				Effect.runPromise(DecodeEligibleRoutineInvocationRequest(changed)),
			).rejects.toBeDefined();
		}
	});

	it("enforces duplicate, collection, and instruction bounds", async () => {
		for (const changed_candidate of [
			{ ...candidate, commands: [command, command] },
			{ ...candidate, files: [file, file] },
			{ ...candidate, permissions: [permission, permission] },
		]) {
			await expect(
				Effect.runPromise(DecodeRoutineInstallCandidate(changed_candidate)),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeRoutineInstructionsResult({
					instructions: {
						content: "a".repeat(65_537),
						content_hash: instructions_reference.content_hash,
					},
					query: { eligibility },
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeRoutineListResult({
					query: list_query,
					routines: Array.from({ length: 129 }, (_, index) => ({
						...eligibility,
						installation: {
							...installation,
							installation_id: `installation_${index}`,
							routine: {
								...routine,
								summary: { ...routine.summary, routine_id: `routine.${index}` },
							},
						},
					})),
				}),
			),
		).rejects.toBeDefined();
	});
});
