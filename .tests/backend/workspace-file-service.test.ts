import { createHash } from "node:crypto";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime, Option } from "effect";
import { describe, expect, it } from "vitest";

import type { ContentIdentity } from "@artisan/protocol";

import { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceChangeRepository } from "../../modules/backend/src/workspace/workspace-change-repository";
import {
	WorkspaceChangeDiffLimit,
	WorkspaceChangeDiffService,
} from "../../modules/backend/src/workspace/workspace-change-diff-service";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
	WorkspaceFileServiceLive,
	type WorkspaceFileReplaceInput,
} from "../../modules/backend/src/workspace/workspace-file-service";
import {
	WorkspaceMutationAuthority,
	WorkspaceMutationAuthorityConflict,
} from "../../modules/backend/src/workspace/workspace-mutation-authority";
import {
	WorkspaceMutationPayloadStore,
	WorkspaceMutationPayloadStoreUnavailable,
} from "../../modules/backend/src/workspace/workspace-mutation-payload-store";
import { WorkspaceSnapshotStore } from "../../modules/backend/src/workspace/workspace-snapshot-store";
import { WorkspaceReplaceApprovalRepository } from "../../modules/backend/src/workspace/workspace-replace-approval-repository";

const encoder = new TextEncoder();
const now = "2026-07-12T12:00:00.000Z";

function identity(bytes: Uint8Array): ContentIdentity {
	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function replacement_input(content = "after"): WorkspaceFileReplaceInput {
	const before = encoder.encode("before");

	return {
		agent_id: "agent_1",
		change_id: "change_1",
		content,
		expected_before: identity(before),
		message_id: "message_1",
		path: "src/example.ts",
		run_id: "run_1",
		sent_at: now,
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function make_runtime(
	options: {
		readonly bytes?: Uint8Array;
		readonly changed_reconciliations?: ReadonlyArray<"applied" | "committed" | "rejected">;
		readonly diff_limit?: boolean;
		readonly finalize_failures?: number;
		readonly replace_result?: "AlreadyReplaced" | "Changed" | "Replaced";
		readonly unavailable_payload_record?: boolean;
	} = {},
) {
	let bytes = new Uint8Array(options.bytes ?? encoder.encode("before"));
	let lifecycle: "applied" | "claimed" | "committed" | "rejected" = "claimed";
	let payload: { expected: Uint8Array; replacement: Uint8Array } | undefined;
	let payload_record_exists = options.unavailable_payload_record ?? false;
	let snapshot: Uint8Array | undefined;
	let finalize_failures = options.finalize_failures ?? 0;
	let reconciliation_calls = 0;
	let replace_calls = 0;
	const claims: Array<unknown> = [];
	const calls: Array<string> = [];
	const evidence: Array<unknown> = [];
	const events: Array<unknown> = [];

	const store = {
		FinalizeRegularFileReplacement: () => {
			calls.push("finalize");

			if (finalize_failures > 0) {
				finalize_failures -= 1;

				return Effect.fail({ _tag: "finalize_failure" });
			}

			return Effect.void;
		},
		ReadRegularFile: () => Effect.succeed(new Uint8Array(bytes)),
		ReplaceRegularFile: (input: { readonly replacement: Uint8Array }) => {
			calls.push("replace");
			replace_calls += 1;

			if (options.replace_result === "Changed")
				return Effect.succeed({ _tag: "Changed" as const });

			bytes = new Uint8Array(input.replacement);

			return Effect.succeed({ _tag: options.replace_result ?? "Replaced" } as const);
		},
	};
	const reader = { ReadRegularFile: store.ReadRegularFile };
	const claim = (input: unknown) => {
		claims.push(input);

		if (lifecycle === "committed") {
			return Effect.succeed({
				authority: { _tag: "base_run", approval: "on_request" },
				claim: {
					_tag: "duplicate",
					event: { event_id: "event_1" },
					operation: { action: "replace", lifecycle },
				},
				store,
			});
		}

		if (lifecycle === "rejected") {
			return Effect.fail(
				new WorkspaceMutationAuthorityConflict({ reason: "operation_rejected" }),
			);
		}

		return Effect.succeed({
			authority: { _tag: "base_run", approval: "on_request" },
			claim: {
				_tag:
					lifecycle === "claimed" && !payload_record_exists
						? "claimed"
						: "incomplete_retry",
				operation: { action: "replace", lifecycle },
			},
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
			Get: () =>
				Effect.succeed({
					reader,
					workspace_id: "workspace_1",
				}),
			ListWorkspaceIds: Effect.succeed(["workspace_1"]),
		} as unknown as typeof WorkspaceBoundedRegularFileStoreRegistry.Service),
		Layer.succeed(WorkspaceMutationAuthority, {
			ClaimReplace: claim,
		} as unknown as typeof WorkspaceMutationAuthority.Service),
		Layer.succeed(WorkspaceReplaceApprovalRepository, {
			Decide: () => Effect.die("unused"),
			ListDeniedUnsettled: Effect.succeed([]),
			ListExecutable: Effect.succeed([]),
			MarkApplied: () => Effect.die("unused"),
			MarkExecuting: () => Effect.die("unused"),
			MarkRejected: () => Effect.die("unused"),
			Query: () => Effect.die("unused"),
			ReadByMessage: () => Effect.succeed(Option.none()),
			ReadDenied: () => Effect.die("unused"),
			ReadExecution: () => Effect.die("unused"),
			Request: () => Effect.die("unused"),
		} as typeof WorkspaceReplaceApprovalRepository.Service),
		Layer.succeed(WorkspaceChangeDiffService, {
			Prepare: (input) => {
				calls.push("prepare");

				if (options.diff_limit) {
					return Effect.fail(new WorkspaceChangeDiffLimit({ limit: "edit_length" }));
				}

				const patch = encoder.encode(
					`--- a/${input.path}\n+++ b/${input.path}\n@@ -1,1 +1,1 @@\n-before\n+after\n`,
				);

				return Effect.succeed({
					added_line_count: 1,
					after_identity: input.after_identity,
					before_identity: input.before_identity,
					change_id: input.change_id,
					context_lines: 3,
					format: "unified" as const,
					format_version: 1,
					message_id: input.message_id,
					patch,
					patch_identity: identity(patch),
					path: input.path,
					removed_line_count: 1,
					thread_id: input.thread_id,
					workspace_id: input.workspace_id,
				});
			},
			Read: () => Effect.die("unused"),
		} as typeof WorkspaceChangeDiffService.Service),
		Layer.succeed(WorkspaceMutationPayloadStore, {
			Consume: () => {
				payload = undefined;
				payload_record_exists = true;

				return Effect.void;
			},
			HasRecord: () => Effect.succeed(payload_record_exists),
			Resume: () =>
				payload === undefined
					? Effect.fail(
							new WorkspaceMutationPayloadStoreUnavailable({
								message_id: "message_1",
								operation: "resume",
							}),
						)
					: Effect.succeed(payload),
			Stage: (input: { readonly expected: Uint8Array; readonly replacement: Uint8Array }) => {
				if (payload_record_exists && payload === undefined) {
					return Effect.fail(
						new WorkspaceMutationPayloadStoreUnavailable({
							message_id: "message_1",
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
			Consume: () => Effect.void,
			DiscardRejectedReplace: () => {
				snapshot = undefined;

				return Effect.void;
			},
			Exists: () => Effect.succeed(snapshot !== undefined),
			Read: () => Effect.succeed(new Uint8Array(snapshot ?? [])),
			Resume: () => Effect.succeed(new Uint8Array(snapshot ?? [])),
			Stage: (input: { readonly content: Uint8Array }) => {
				calls.push("snapshot");
				snapshot ??= new Uint8Array(input.content);

				return Effect.succeed({ status: "staged" as const });
			},
		} as unknown as typeof WorkspaceSnapshotStore.Service),
		Layer.succeed(WorkspaceChangeRepository, {
			ClaimReplace: () => Effect.die("unused"),
			ClaimReview: () => Effect.die("unused"),
			ClaimRollback: () => Effect.die("unused"),
			CommitRecorded: () => {
				calls.push("commit");
				lifecycle = "committed";
				events.push({ type: "workspace.change.updated" });

				return Effect.succeed({
					event: { event_id: "event_1" },
					status: "accepted" as const,
				});
			},
			CommitReviewed: () => Effect.die("unused"),
			CommitRolledBack: () => Effect.die("unused"),
			List: () => Effect.die("unused"),
			MarkApplied: () => {
				calls.push("mark_applied");
				lifecycle = "applied";

				return Effect.succeed({ lifecycle });
			},
			MarkEvidenceRecorded: () => Effect.succeed({ lifecycle }),
			ReadChange: () => Effect.die("unused"),
			ReadOperation: () => Effect.die("unused"),
			ReconcileChanged: () => {
				reconciliation_calls += 1;
				const reconciliation = options.changed_reconciliations?.[reconciliation_calls - 1];

				if (reconciliation) {
					lifecycle = reconciliation;

					return Effect.succeed(
						reconciliation === "committed"
							? {
									_tag: "committed" as const,
									event: { event_id: "event_1" },
									operation: { lifecycle },
								}
							: { _tag: reconciliation, operation: { lifecycle } },
					);
				}

				if (lifecycle === "committed") {
					return Effect.succeed({
						_tag: "committed" as const,
						event: { event_id: "event_1" },
						operation: { lifecycle },
					});
				}

				if (lifecycle === "applied") {
					return Effect.succeed({
						_tag: "applied" as const,
						operation: { lifecycle },
					});
				}

				lifecycle = "rejected";

				return Effect.succeed({
					_tag: "rejected" as const,
					operation: { lifecycle },
				});
			},
			RejectChanged: () => {
				lifecycle = "rejected";

				return Effect.succeed({ lifecycle });
			},
		} as unknown as typeof WorkspaceChangeRepository.Service),
		Layer.succeed(WorkspaceEvidenceRecorder, {
			RecordFilesystemMutation: (input: unknown) => {
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
			bytes: () => new TextDecoder().decode(bytes),
			calls,
			evidence,
			claims,
			events,
			lifecycle: () => lifecycle,
			reconciliation_calls: () => reconciliation_calls,
			replace_calls: () => replace_calls,
			reader,
			snapshot: () => snapshot && new TextDecoder().decode(snapshot),
		},
		runtime: ManagedRuntime.make(WorkspaceFileServiceLive.pipe(Layer.provide(layers))),
	};
}

describe("WorkspaceFileService", () => {
	it("reads through the restricted registry reader", async () => {
		const harness = make_runtime();

		try {
			expect("ReplaceRegularFile" in harness.state.reader).toBe(false);
			expect("FinalizeRegularFileReplacement" in harness.state.reader).toBe(false);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("strictly reads bounded UTF-8 content and computes its identity", async () => {
		const harness = make_runtime();

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.gen(function* () {
						return yield* (yield* WorkspaceFileService).Read({
							path: "src/example.ts",
							workspace_id: "workspace_1",
						});
					}),
				),
			).resolves.toMatchObject({ content: "before", identity: { byte_count: 6 } });
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("conceals malformed UTF-8 read failures", async () => {
		const harness = make_runtime({ bytes: new Uint8Array([0xc3, 0x28]) });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Read({ path: "src/example.ts", workspace_id: "workspace_1" }),
					),
				),
			).rejects.toEqual(
				new WorkspaceFileServiceError({ operation: "read", reason: "failed" }),
			);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("records a replacement, preserves its private snapshot, and attributes evidence", async () => {
		const harness = make_runtime();

		try {
			await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(replacement_input()),
				),
			);

			expect(harness.state.bytes()).toBe("after");
			expect(harness.state.snapshot()).toBe("before");
			expect(harness.state.events).toHaveLength(1);
			expect(harness.state.evidence).toMatchObject([{ operation: "write", run_id: "run_1" }]);
			expect(harness.state.calls).toEqual([
				"prepare",
				"snapshot",
				"replace",
				"mark_applied",
				"finalize",
				"commit",
			]);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rejects a diff over the V1 budget before snapshotting or replacing", async () => {
		const harness = make_runtime({ diff_limit: true });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toEqual(
				new WorkspaceFileServiceError({ operation: "replace", reason: "diff_limit" }),
			);
			expect(harness.state.calls).toEqual(["prepare"]);
			expect(harness.state.lifecycle()).toBe("rejected");
			expect(harness.state.bytes()).toBe("before");
			expect(harness.state.snapshot()).toBeUndefined();
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.events).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("terminally rejects mismatched content without a snapshot, event, or evidence", async () => {
		const harness = make_runtime({ bytes: encoder.encode("external") });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toMatchObject({ operation: "replace", reason: "changed" });

			expect(harness.state.snapshot()).toBeUndefined();
			expect(harness.state.events).toHaveLength(0);
			expect(harness.state.evidence).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("finishes duplicate replacement cleanup when payload consumption wins after preflight", async () => {
		const harness = make_runtime({
			bytes: encoder.encode("after"),
			changed_reconciliations: ["applied", "committed"],
		});

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).resolves.toMatchObject({ status: "duplicate" });
			expect(harness.state.reconciliation_calls()).toBe(2);
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.evidence).toHaveLength(1);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("never exposes workspace source or paths through replacement failures", async () => {
		const harness = make_runtime({ bytes: encoder.encode("private external source") });
		const input = {
			...replacement_input("private intended source"),
			path: "private/absolute-looking/source.ts",
		};

		try {
			const failure = await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(input).pipe(Effect.flip),
				),
			);
			const serialized = JSON.stringify(failure);

			expect(serialized).not.toContain("private external source");
			expect(serialized).not.toContain("private intended source");
			expect(serialized).not.toContain(input.path);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("settles an exact rejected retry without receiving or using a mutation store", async () => {
		const harness = make_runtime({ replace_result: "Changed" });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toMatchObject({ operation: "replace", reason: "changed" });
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toMatchObject({ operation: "replace", reason: "changed" });

			expect(harness.state.replace_calls()).toBe(1);
			expect(harness.state.events).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fails closed on an unavailable claimed payload record instead of restaging", async () => {
		const harness = make_runtime({ unavailable_payload_record: true });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toMatchObject({ operation: "replace", reason: "failed" });

			expect(harness.state.lifecycle()).toBe("claimed");
			expect(harness.state.replace_calls()).toBe(0);
			expect(harness.state.events).toHaveLength(0);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("fingerprints maximum-sized source through its identity rather than JSON source bytes", async () => {
		const harness = make_runtime();

		try {
			await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(replacement_input("a".repeat(4 * 1024 * 1024))),
				),
			);

			expect(harness.state.claims[0]).toMatchObject({
				request_fingerprint: expect.stringMatching(/^[a-f0-9]{64}$/),
			});
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("does not replace again after an exact committed retry", async () => {
		const harness = make_runtime();

		try {
			await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(replacement_input()),
				),
			);
			await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(replacement_input()),
				),
			);

			expect(harness.state.replace_calls()).toBe(1);
			expect(harness.state.evidence).toHaveLength(2);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("resumes an applied replacement after finalization fails without replacing again", async () => {
		const harness = make_runtime({ finalize_failures: 1 });

		try {
			await expect(
				harness.runtime.runPromise(
					Effect.flatMap(WorkspaceFileService, (service) =>
						service.Replace(replacement_input()),
					),
				),
			).rejects.toMatchObject({ operation: "replace", reason: "failed" });
			await harness.runtime.runPromise(
				Effect.flatMap(WorkspaceFileService, (service) =>
					service.Replace(replacement_input()),
				),
			);

			expect(harness.state.replace_calls()).toBe(1);
			expect(harness.state.events).toHaveLength(1);
		} finally {
			await harness.runtime.dispose();
		}
	});
});
