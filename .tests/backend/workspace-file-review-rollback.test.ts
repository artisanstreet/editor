import { createHash } from "node:crypto";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import type { ContentIdentity } from "@artisan/protocol";

import { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceChangeRepository } from "../../modules/backend/src/workspace/workspace-change-repository";
import { WorkspaceChangeDiffService } from "../../modules/backend/src/workspace/workspace-change-diff-service";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
	WorkspaceFileServiceLive,
	type WorkspaceFileReviewInput,
	type WorkspaceFileRollbackInput,
} from "../../modules/backend/src/workspace/workspace-file-service";
import { WorkspaceMutationAuthority } from "../../modules/backend/src/workspace/workspace-mutation-authority";
import {
	WorkspaceMutationPayloadStore,
	WorkspaceMutationPayloadStoreUnavailable,
} from "../../modules/backend/src/workspace/workspace-mutation-payload-store";
import { WorkspaceSnapshotStore } from "../../modules/backend/src/workspace/workspace-snapshot-store";
import { WorkspaceReplaceApprovalRepository } from "../../modules/backend/src/workspace/workspace-replace-approval-repository";

const encoder = new TextEncoder();
const now = "2026-07-12T12:00:00.000Z";

function identity(content: string): ContentIdentity {
	const bytes = encoder.encode(content);

	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function review_input(): WorkspaceFileReviewInput {
	return {
		change_id: "change_1",
		message_id: "review_1",
		sent_at: now,
		thread_id: "thread_1",
	};
}

function rollback_input(): WorkspaceFileRollbackInput {
	return {
		change_id: "change_1",
		expected_after: identity("request-after"),
		message_id: "rollback_1",
		sent_at: now,
		thread_id: "thread_1",
	};
}

function rollback_source() {
	return {
		after_identity: identity("after"),
		before_identity: identity("before"),
		path: "authority-bound/example.ts",
		workspace_id: "workspace_1",
	};
}

function make_harness(
	options: {
		readonly changed_reconciliation?: "applied" | "committed" | "rejected" | "staged";
		readonly changed_reconciliations?: ReadonlyArray<
			"applied" | "committed" | "rejected" | "staged"
		>;
		readonly current?: string;
		readonly finalize_failures?: number;
		readonly payload_record_exists?: boolean;
		readonly replace_result?: "Changed" | "Replaced";
		readonly rollback_lifecycle?:
			| "applied"
			| "claimed"
			| "committed"
			| "incomplete"
			| "rejected";
		readonly review_claim?: "claimed" | "duplicate" | "incomplete_retry";
	} = {},
) {
	let current = options.current ?? "after";
	let finalize_failures = options.finalize_failures ?? 0;
	let payload:
		| {
				readonly expected: Uint8Array;
				readonly replacement: Uint8Array;
		  }
		| undefined;
	let payload_record_exists = options.payload_record_exists ?? false;
	let rollback_lifecycle = options.rollback_lifecycle ?? "claimed";
	const changed_reconciliation = options.changed_reconciliation ?? "rejected";
	let changed_reconciliation_index = 0;
	let snapshot_available = true;
	let review_claim = options.review_claim ?? "claimed";
	const calls: string[] = [];
	const claims: unknown[] = [];
	const evidence: Array<Record<string, unknown>> = [];
	const file_read_paths: string[] = [];
	const mutation_paths: string[] = [];
	const payload_consumes: unknown[] = [];
	const stage_inputs: Array<Record<string, unknown>> = [];
	let file_read_calls = 0;
	let replace_calls = 0;
	let snapshot_read_calls = 0;
	let stage_calls = 0;

	const store = {
		FinalizeRegularFileReplacement: (input: { readonly path: string }) => {
			calls.push("finalize");
			mutation_paths.push(input.path);

			if (finalize_failures > 0) {
				finalize_failures -= 1;

				return Effect.fail({ _tag: "finalize_failure" });
			}

			return Effect.void;
		},
		ReadRegularFile: (path: string) => {
			file_read_calls += 1;
			file_read_paths.push(path);

			return Effect.succeed(encoder.encode(current));
		},
		ReplaceRegularFile: (input: {
			readonly path: string;
			readonly replacement: Uint8Array;
		}) => {
			replace_calls += 1;
			calls.push("replace");
			mutation_paths.push(input.path);

			if (options.replace_result === "Changed") {
				return Effect.succeed({ _tag: "Changed" as const });
			}

			current = new TextDecoder().decode(input.replacement);

			return Effect.succeed({ _tag: "Replaced" as const });
		},
	};
	const rollback_admission = () => {
		if (rollback_lifecycle === "rejected") {
			return Effect.succeed({
				_tag: "rejected" as const,
				authority: { _tag: "base_run" as const },
				claim: {
					_tag: "rejected" as const,
					operation: { lifecycle: "rejected" },
				},
				source: rollback_source(),
			});
		}

		if (rollback_lifecycle === "committed") {
			return Effect.succeed({
				_tag: "duplicate" as const,
				authority: { _tag: "base_run" as const },
				claim: {
					_tag: "duplicate" as const,
					event: { event_id: "event_1" },
					operation: { lifecycle: "committed" },
				},
				source: rollback_source(),
			});
		}

		return Effect.succeed({
			_tag: "authorized" as const,
			authority: { _tag: "base_run" as const },
			claim: {
				_tag: rollback_lifecycle === "claimed" ? "claimed" : "incomplete_retry",
				operation: {
					lifecycle: rollback_lifecycle === "incomplete" ? "claimed" : rollback_lifecycle,
				},
			},
			source: rollback_source(),
			store,
		});
	};
	const layers = Layer.mergeAll(
		Layer.succeed(
			BoundedRegularFileStore,
			store as unknown as typeof BoundedRegularFileStore.Service,
		),
		Layer.succeed(WorkspaceBoundedRegularFileStoreRegistry, {
			Authorize: () => Effect.die("unused"),
			Get: () => Effect.die("review and rollback never use the registry"),
			ListWorkspaceIds: Effect.succeed([]),
		} as unknown as typeof WorkspaceBoundedRegularFileStoreRegistry.Service),
		Layer.succeed(WorkspaceMutationAuthority, {
			ClaimReplace: () => Effect.die("unused"),
			ClaimRollback: (input: unknown) => {
				claims.push(input);

				return rollback_admission();
			},
		} as unknown as typeof WorkspaceMutationAuthority.Service),
		Layer.succeed(WorkspaceReplaceApprovalRepository, {
			Decide: () => Effect.die("unused"),
			ListDeniedUnsettled: Effect.succeed([]),
			ListExecutable: Effect.succeed([]),
			MarkApplied: () => Effect.die("unused"),
			MarkExecuting: () => Effect.die("unused"),
			MarkRejected: () => Effect.die("unused"),
			Query: () => Effect.die("unused"),
			ReadByMessage: () => Effect.die("unused"),
			ReadDenied: () => Effect.die("unused"),
			ReadExecution: () => Effect.die("unused"),
			Request: () => Effect.die("unused"),
		} as typeof WorkspaceReplaceApprovalRepository.Service),
		Layer.succeed(WorkspaceChangeDiffService, {
			Prepare: () => Effect.die("unused"),
			Read: () => Effect.die("unused"),
		} as typeof WorkspaceChangeDiffService.Service),
		Layer.succeed(WorkspaceMutationPayloadStore, {
			Consume: (input: unknown) => {
				calls.push("payload_consume");
				payload_consumes.push(input);
				payload = undefined;
				payload_record_exists = true;

				return Effect.void;
			},
			HasRecord: () => Effect.succeed(payload_record_exists),
			Resume: () =>
				payload === undefined
					? Effect.fail(
							new WorkspaceMutationPayloadStoreUnavailable({
								message_id: "rollback_1",
								operation: "resume",
							}),
						)
					: Effect.succeed(payload),
			Stage: (input: {
				readonly expected: Uint8Array;
				readonly expected_identity: ContentIdentity;
				readonly replacement: Uint8Array;
				readonly replacement_identity: ContentIdentity;
			}) => {
				stage_calls += 1;
				calls.push("payload_stage");
				stage_inputs.push(input);

				if (payload_record_exists && payload === undefined) {
					return Effect.fail(
						new WorkspaceMutationPayloadStoreUnavailable({
							message_id: "rollback_1",
							operation: "stage",
						}),
					);
				}

				payload ??= {
					expected: new Uint8Array(input.expected),
					replacement: new Uint8Array(input.replacement),
				};
				payload_record_exists = true;

				return Effect.succeed({ status: "staged" as const });
			},
		} as unknown as typeof WorkspaceMutationPayloadStore.Service),
		Layer.succeed(WorkspaceSnapshotStore, {
			Consume: () => {
				calls.push("snapshot_consume");
				snapshot_available = false;

				return Effect.void;
			},
			DiscardRejectedReplace: () => Effect.die("unused"),
			Exists: () => Effect.succeed(snapshot_available),
			Read: () => {
				snapshot_read_calls += 1;

				return snapshot_available
					? Effect.succeed(encoder.encode("before"))
					: Effect.die("snapshot unavailable");
			},
			Resume: () => Effect.die("unused"),
			Stage: () => Effect.die("unused"),
		} as unknown as typeof WorkspaceSnapshotStore.Service),
		Layer.succeed(WorkspaceChangeRepository, {
			ClaimReplace: () => Effect.die("unused"),
			ClaimReview: (input: unknown) => {
				claims.push(input);

				return Effect.succeed(
					review_claim === "duplicate"
						? {
								_tag: "duplicate" as const,
								event: { event_id: "event_1" },
								operation: { lifecycle: "committed" },
							}
						: {
								_tag: review_claim,
								operation: { lifecycle: "claimed" },
							},
				);
			},
			ClaimRollback: () => Effect.die("unused"),
			CommitRecorded: () => Effect.die("unused"),
			CommitReviewed: () => {
				calls.push("review_commit");
				review_claim = "duplicate";

				return Effect.succeed({
					event: { event_id: "event_1" },
					status: "accepted" as const,
				});
			},
			CommitRolledBack: () => {
				calls.push("rollback_commit");
				rollback_lifecycle = "committed";

				return Effect.succeed({
					event: { event_id: "event_1" },
					status: "accepted" as const,
				});
			},
			List: () => Effect.die("unused"),
			MarkApplied: () => {
				calls.push("mark_applied");
				rollback_lifecycle = "applied";

				return Effect.succeed({ lifecycle: "applied" });
			},
			MarkEvidenceRecorded: () => {
				calls.push("mark_evidence");

				return Effect.succeed({ lifecycle: rollback_lifecycle });
			},
			ReadChange: () => Effect.die("rollback source must come from authority"),
			ReadOperation: () => Effect.die("unused"),
			ReconcileChanged: () => {
				calls.push("reconcile_changed");
				const reconciliation =
					options.changed_reconciliations?.[changed_reconciliation_index++] ??
					changed_reconciliation;

				if (reconciliation === "staged") {
					payload = {
						expected: encoder.encode("after"),
						replacement: encoder.encode("before"),
					};
					payload_record_exists = true;

					return Effect.succeed({
						_tag: "staged" as const,
						operation: { lifecycle: rollback_lifecycle },
					});
				}

				rollback_lifecycle = reconciliation;

				return Effect.succeed(
					reconciliation === "committed"
						? {
								_tag: "committed" as const,
								event: { event_id: "event_1" },
								operation: { lifecycle: "committed" },
							}
						: {
								_tag: reconciliation,
								operation: { lifecycle: reconciliation },
							},
				);
			},
			RejectChanged: () => {
				calls.push("reject_changed");
				rollback_lifecycle = "rejected";

				return Effect.succeed({ lifecycle: "rejected" });
			},
		} as unknown as typeof WorkspaceChangeRepository.Service),
		Layer.succeed(WorkspaceEvidenceRecorder, {
			RecordFilesystemMutation: (input: Record<string, unknown>) => {
				calls.push("evidence");
				evidence.push(input);

				return Effect.succeed({
					event: { event_id: "evidence_1" },
					status: "accepted" as const,
				});
			},
			RecordGitWorkspaceObserved: () => Effect.die("unused"),
			RecordProcessOwnership: () => Effect.die("unused"),
		} as unknown as typeof WorkspaceEvidenceRecorder.Service),
		NodeCrypto.layer,
	);

	return {
		state: {
			calls,
			claims,
			current: () => current,
			evidence,
			file_read_calls: () => file_read_calls,
			file_read_paths,
			mutation_paths,
			payload_consumes,
			payload_exists: () => payload_record_exists,
			replace_calls: () => replace_calls,
			snapshot_read_calls: () => snapshot_read_calls,
			snapshot_available: () => snapshot_available,
			stage_calls: () => stage_calls,
			stage_inputs,
		},
		runtime: ManagedRuntime.make(WorkspaceFileServiceLive.pipe(Layer.provide(layers))),
	};
}

function run_review(harness: ReturnType<typeof make_harness>) {
	return harness.runtime.runPromise(
		Effect.flatMap(WorkspaceFileService, (service) => service.Review(review_input())),
	);
}

function run_rollback(harness: ReturnType<typeof make_harness>) {
	return harness.runtime.runPromise(
		Effect.flatMap(WorkspaceFileService, (service) => service.Rollback(rollback_input())),
	);
}

describe("WorkspaceFileService review and rollback", () => {
	it("reviews accepted and incomplete retries without a filesystem capability", async () => {
		const harness = make_harness({ review_claim: "incomplete_retry" });

		try {
			await expect(run_review(harness)).resolves.toMatchObject({ status: "accepted" });
			expect(harness.state.claims[0]).toMatchObject({
				_tag: "review",
				request_fingerprint: expect.stringMatching(/^[a-f0-9]{64}$/),
			});
			expect(harness.state.calls).toEqual(["review_commit"]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("returns a review duplicate exactly", async () => {
		const harness = make_harness({ review_claim: "duplicate" });

		try {
			await expect(run_review(harness)).resolves.toMatchObject({ status: "duplicate" });
			expect(harness.state.calls).toEqual([]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rolls back, consumes the snapshot only after commit, and records anonymous evidence", async () => {
		const harness = make_harness();

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "accepted" });
			expect(harness.state.current()).toBe("before");
			expect(harness.state.file_read_paths).toEqual(["authority-bound/example.ts"]);
			expect(harness.state.mutation_paths).toEqual([
				"authority-bound/example.ts",
				"authority-bound/example.ts",
			]);
			expect(harness.state.stage_inputs[0]).toMatchObject({
				expected_identity: rollback_source().after_identity,
				replacement_identity: rollback_source().before_identity,
			});
			expect(harness.state.calls).toEqual([
				"payload_stage",
				"replace",
				"mark_applied",
				"finalize",
				"rollback_commit",
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
			expect(harness.state.evidence).toEqual([
				{
					operation: "write",
					operation_id: "rollback_1",
					path: "authority-bound/example.ts",
					thread_id: "thread_1",
				},
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rejects a changed current file, settles only its payload, and retains the snapshot", async () => {
		const harness = make_harness({ current: "external" });

		try {
			await expect(run_rollback(harness)).rejects.toEqual(
				new WorkspaceFileServiceError({ operation: "rollback", reason: "changed" }),
			);
			expect(harness.state.snapshot_available()).toBe(true);
			expect(harness.state.file_read_calls()).toBe(1);
			expect(harness.state.snapshot_read_calls()).toBe(1);
			expect(harness.state.stage_calls()).toBe(0);
			expect(harness.state.calls).toEqual(["reconcile_changed", "payload_consume"]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("continues rollback when a concurrent caller staged the published preflight", async () => {
		const harness = make_harness({
			changed_reconciliation: "staged",
			current: "before",
		});

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "accepted" });
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.stage_calls()).toBe(0);
			expect(harness.state.calls).toEqual([
				"reconcile_changed",
				"replace",
				"mark_applied",
				"finalize",
				"rollback_commit",
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("finishes duplicate rollback cleanup when payload consumption wins after preflight", async () => {
		const harness = make_harness({
			changed_reconciliations: ["applied", "committed"],
			current: "before",
		});

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "duplicate" });
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.calls).toEqual([
				"reconcile_changed",
				"reconcile_changed",
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rejects native changed without consuming the original snapshot", async () => {
		const harness = make_harness({ replace_result: "Changed" });

		try {
			await expect(run_rollback(harness)).rejects.toMatchObject({
				operation: "rollback",
				reason: "changed",
			});
			expect(harness.state.snapshot_available()).toBe(true);
			expect(harness.state.file_read_calls()).toBe(1);
			expect(harness.state.replace_calls()).toBe(1);
			expect(harness.state.snapshot_read_calls()).toBe(1);
			expect(harness.state.stage_calls()).toBe(1);
			expect(harness.state.payload_exists()).toBe(true);
			expect(harness.state.calls).toEqual([
				"payload_stage",
				"replace",
				"reconcile_changed",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("finishes applied rollback recovery after native changed", async () => {
		const harness = make_harness({
			changed_reconciliation: "applied",
			replace_result: "Changed",
		});

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "accepted" });
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.calls).toEqual([
				"payload_stage",
				"replace",
				"reconcile_changed",
				"finalize",
				"rollback_commit",
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("finishes committed rollback cleanup after native changed", async () => {
		const harness = make_harness({
			changed_reconciliation: "committed",
			replace_result: "Changed",
		});

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "duplicate" });
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.calls).toEqual([
				"payload_stage",
				"replace",
				"reconcile_changed",
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("settles an exact rejected retry from its bound source without filesystem access", async () => {
		const harness = make_harness({ rollback_lifecycle: "rejected" });

		try {
			await expect(run_rollback(harness)).rejects.toEqual(
				new WorkspaceFileServiceError({ operation: "rollback", reason: "changed" }),
			);
			expect(harness.state.payload_consumes).toMatchObject([
				{
					expected_identity: rollback_source().after_identity,
					replacement_identity: rollback_source().before_identity,
				},
			]);
			expect(harness.state.calls).toEqual(["payload_consume"]);
			expect(harness.state.file_read_calls()).toBe(0);
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.snapshot_read_calls()).toBe(0);
			expect(harness.state.snapshot_available()).toBe(true);
			expect(harness.state.stage_calls()).toBe(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("cleans up a committed duplicate without another replacement", async () => {
		const harness = make_harness({ rollback_lifecycle: "committed" });

		try {
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "duplicate" });
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.file_read_calls()).toBe(0);
			expect(harness.state.snapshot_read_calls()).toBe(0);
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.stage_calls()).toBe(0);
			expect(harness.state.calls).toEqual([
				"snapshot_consume",
				"evidence",
				"mark_evidence",
				"payload_consume",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("recovers applied finalization without a second replacement", async () => {
		const harness = make_harness({ finalize_failures: 1 });

		try {
			await expect(run_rollback(harness)).rejects.toMatchObject({
				operation: "rollback",
				reason: "failed",
			});
			await expect(run_rollback(harness)).resolves.toMatchObject({ status: "accepted" });
			expect(harness.state.replace_calls()).toBe(1);
			expect(harness.state.file_read_calls()).toBe(1);
			expect(harness.state.snapshot_read_calls()).toBe(1);
			expect(harness.state.snapshot_available()).toBe(false);
			expect(harness.state.stage_calls()).toBe(1);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fails closed when an unavailable payload record is still present", async () => {
		const harness = make_harness({
			payload_record_exists: true,
			rollback_lifecycle: "incomplete",
		});

		try {
			await expect(run_rollback(harness)).rejects.toMatchObject({
				operation: "rollback",
				reason: "failed",
			});
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.file_read_calls()).toBe(0);
			expect(harness.state.snapshot_read_calls()).toBe(0);
			expect(harness.state.snapshot_available()).toBe(true);
			expect(harness.state.stage_calls()).toBe(0);
		} finally {
			await harness.runtime.dispose();
		}
	});
});
