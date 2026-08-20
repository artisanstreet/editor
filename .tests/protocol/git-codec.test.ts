import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	GitMutationProjection,
	GitWorkspaceProjection,
	git_diff_maximum_bytes,
	type GitBranchState,
	type GitMutationLifecycle,
} from "@artisan/protocol";

const timestamp = "2026-07-18T08:00:00.000Z";
const head = "a".repeat(40);
const snapshot_id = "b".repeat(64);

function frontend_envelope(kind: string, payload: unknown) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function backend_envelope(kind: string, payload: unknown) {
	return {
		correlation_id: `correlation_${kind}`,
		kind,
		message_id: `message_${kind}`,
		origin: "backend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function summary(overrides: Record<string, number> = {}) {
	return {
		binary_file_count: 0,
		lines_added: 1,
		lines_deleted: 1,
		tracked_file_count: 1,
		...overrides,
	};
}

function repository(branch: GitBranchState = { name: "main", type: "attached" }) {
	const has_head = branch.type !== "unborn";

	return {
		aggregate: summary({ tracked_file_count: 3 }),
		branch,
		clean: false,
		files: [
			{
				flags: { conflicted: false, staged: true, unstaged: false, untracked: false },
				path: "src/staged.ts",
				porcelain_status: "M.",
			},
			{
				flags: { conflicted: false, staged: false, unstaged: true, untracked: false },
				path: "src/unstaged.ts",
				porcelain_status: ".M",
			},
			{
				flags: { conflicted: false, staged: false, unstaged: true, untracked: true },
				path: "src/untracked.ts",
				porcelain_status: "??",
			},
			{
				flags: { conflicted: true, staged: true, unstaged: true, untracked: false },
				path: "src/conflicted.ts",
				porcelain_status: "UU",
			},
		],
		...(has_head ? { head } : {}),
		journal_sequence: 7,
		observed_at: timestamp,
		repository_state: "repository" as const,
		snapshot_id,
		staged: summary(),
		unstaged: summary({ tracked_file_count: 2 }),
		version: 3,
		workspace_id: "workspace_1",
		worktrees: [
			{
				bare: false,
				branch,
				...(has_head ? { head } : {}),
				is_current: true,
				locked: false,
				path: "C:/repo",
				prunable: false,
				worktree_id: "worktree_1",
			},
		],
	};
}

function not_repository() {
	return {
		journal_sequence: 2,
		observed_at: timestamp,
		repository_state: "not_repository" as const,
		snapshot_id,
		version: 1,
		workspace_id: "workspace_1",
	};
}

function mutation(state: GitMutationLifecycle = "awaiting_approval") {
	const base = {
		agent_id: "agent_1",
		approval_id: "approval_1",
		expected_snapshot_id: snapshot_id,
		expected_workspace_version: 3,
		journal_sequence: 8,
		kind: "stage" as const,
		lifecycle: state,
		mutation_id: "mutation_1",
		paths: ["src/staged.ts"],
		raw_origin: { provider: "codex", reference: "item_1" },
		requested_at: timestamp,
		run_id: "run_1",
		source_message_id: "request_1",
		thread_id: "thread_1",
		updated_at: timestamp,
		workspace_id: "workspace_1",
	};
	const decision = {
		decision_at: timestamp,
		decision_message_id: "resolution_1",
	};

	switch (state) {
		case "awaiting_approval":
			return base;
		case "denied":
			return { ...base, ...decision, completed_at: timestamp };
		case "approved":
			return { ...base, ...decision };
		case "dispatching":
			return { ...base, ...decision, dispatched_at: timestamp };
		case "succeeded":
			return {
				...base,
				...decision,
				completed_at: timestamp,
				dispatched_at: timestamp,
				result_snapshot_id: "c".repeat(64),
				result_workspace_version: 4,
			};
		case "failed":
		case "ambiguous":
			return {
				...base,
				...decision,
				completed_at: timestamp,
				dispatched_at: timestamp,
				failure: { code: `git.${state}` },
			};
	}
}

function attributed_mutation_envelope(kind: string, payload: unknown) {
	return {
		...frontend_envelope(kind, payload),
		agent_id: "agent_1",
		raw_origin: { provider: "codex", reference: "item_1" },
		run_id: "run_1",
		thread_id: "thread_1",
	};
}

function mutation_request() {
	return {
		approval_id: "approval_1",
		expected_snapshot_id: snapshot_id,
		expected_workspace_version: 3,
		mutation_id: "mutation_1",
		paths: ["src/staged.ts", "src/space and-æ.ts"],
		workspace_id: "workspace_1",
	};
}

function event(payload: unknown, journal_sequence = 8) {
	return {
		causation_id: "request_1",
		correlation_id: "correlation_1",
		journal_sequence,
		kind: "event",
		message_id: `event_${journal_sequence}`,
		origin: "backend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sequence: journal_sequence,
		sent_at: timestamp,
		stream_id: "workspace:workspace_1",
		thread_id: "thread_1",
	};
}

describe("Git protocol codec", () => {
	it("roundtrips every Git request and correlated query result", async () => {
		const patch = "diff --git a/src/staged.ts b/src/staged.ts\n";
		const inbound = [
			frontend_envelope("git.workspace.query", {
				thread_id: "thread_1",
				workspace_id: "workspace_1",
			}),
			frontend_envelope("git.diff.query", {
				expected_snapshot_id: snapshot_id,
				expected_workspace_version: 3,
				max_bytes: 4_096,
				scope: "staged",
				workspace_id: "workspace_1",
			}),
			attributed_mutation_envelope("git.index.stage.request", mutation_request()),
			attributed_mutation_envelope("git.index.unstage.request", {
				...mutation_request(),
				mutation_id: "mutation_2",
			}),
			attributed_mutation_envelope("git.mutation.resolve", {
				approval_id: "approval_1",
				approved: true,
				mutation_id: "mutation_1",
			}),
		];
		const outbound = [
			backend_envelope("git.workspace.query.result", {
				journal_sequence: 8,
				pending_mutations: [mutation()],
				workspace: repository(),
			}),
			backend_envelope("git.diff.query.result", {
				byte_count: new TextEncoder().encode(patch).byteLength,
				format: "unified",
				format_version: 1,
				patch,
				scope: "staged",
				snapshot_id,
				truncated: false,
				workspace_id: "workspace_1",
				workspace_version: 3,
			}),
		];

		for (const envelope of inbound) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		for (const envelope of outbound) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}
	});

	it("decodes every unresolved Git mutation without a queue-cardinality ceiling", async () => {
		const pending_mutations = Array.from({ length: 10_001 }, (_, index) => ({
			...mutation(),
			mutation_id: `mutation_${String(index)}`,
		}));
		const envelope = backend_envelope("git.workspace.query.result", {
			journal_sequence: 8,
			pending_mutations,
			workspace: repository(),
		});

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it.each([
		{ name: "feature/codec", type: "attached" } as const,
		{ type: "detached" } as const,
		{ name: "main", type: "unborn" } as const,
	])("decodes the $type branch projection", async (branch) => {
		const envelope = backend_envelope("git.workspace.query.result", {
			journal_sequence: 8,
			pending_mutations: [],
			workspace: repository(branch),
		});

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it("decodes not_repository without inventing Git facts", async () => {
		const envelope = backend_envelope("git.workspace.query.result", {
			journal_sequence: 2,
			pending_mutations: [],
			workspace: not_repository(),
		});

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it("represents an existing bare repository without granting current-worktree authority", async () => {
		const workspace = repository();
		const envelope = backend_envelope("git.workspace.query.result", {
			journal_sequence: 8,
			pending_mutations: [],
			workspace: {
				...workspace,
				worktrees: [
					...workspace.worktrees,
					{
						bare: true,
						is_current: false,
						locked: false,
						path: "C:/bare-repo",
						prunable: false,
						worktree_id: "worktree_bare",
					},
				],
			},
		});

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it.each([
		"awaiting_approval",
		"denied",
		"approved",
		"dispatching",
		"succeeded",
		"failed",
		"ambiguous",
	] as const)("decodes the %s durable mutation lifecycle", async (state) => {
		const envelope = event({ mutation: mutation(state), type: "git.mutation.updated" });

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it.each(["refresh", "mutation", "recovery"] as const)(
		"decodes a Git workspace event caused by %s",
		async (cause) => {
			const envelope = event(
				{ cause, type: "git.workspace.updated", workspace: repository() },
				7,
			);

			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		},
	);

	it("allows the bounded diff query result to report deliberate truncation", async () => {
		const envelope = backend_envelope("git.diff.query.result", {
			byte_count: 4,
			format: "unified",
			format_version: 1,
			patch: "diff",
			scope: "aggregate",
			snapshot_id,
			truncated: true,
			workspace_id: "workspace_1",
			workspace_version: 3,
		});

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it.each([
		"/src/main.ts",
		"C:/repo/src/main.ts",
		"C:src/main.ts",
		"../src/main.ts",
		"src/../main.ts",
		"src//main.ts",
		"src/\u0000main.ts",
	])("rejects the non-canonical mutation path %j", async (path) => {
		const envelope = attributed_mutation_envelope("git.index.stage.request", {
			...mutation_request(),
			paths: [path],
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each(["src\\literal-backslash.ts", "src/line\nbreak.ts", "src/control-\u0001.ts"])(
		"preserves the valid odd Git mutation path %j",
		async (path) => {
			const envelope = attributed_mutation_envelope("git.index.stage.request", {
				...mutation_request(),
				paths: [path],
			});

			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		},
	);

	it("rejects a Git path whose UTF-8 representation exceeds its public bound", async () => {
		const envelope = attributed_mutation_envelope("git.index.stage.request", {
			...mutation_request(),
			paths: ["a".repeat(16 * 1024 + 1)],
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects duplicate exact mutation paths", async () => {
		const envelope = attributed_mutation_envelope("git.index.stage.request", {
			...mutation_request(),
			paths: ["src/main.ts", "src/main.ts"],
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects mutation path sets that exceed bounded subprocess stdin", async () => {
		const envelope = attributed_mutation_envelope("git.index.stage.request", {
			...mutation_request(),
			paths: Array.from({ length: 300 }, (_, index) => `${index}-${"a".repeat(3_990)}`),
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each(["expected_snapshot_id", "expected_workspace_version"] as const)(
		"requires mutation optimistic-concurrency field %s",
		async (field) => {
			const payload = mutation_request();
			delete payload[field];

			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope(
						attributed_mutation_envelope("git.index.stage.request", payload),
					),
				),
			).rejects.toBeDefined();
		},
	);

	it("requires thread attribution while keeping run, agent, and raw origin optional", async () => {
		const valid = {
			...frontend_envelope("git.index.stage.request", mutation_request()),
			thread_id: "thread_1",
		};
		const invalid = { ...valid };
		delete (invalid as Partial<typeof invalid>).thread_id;

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(valid))).resolves.toEqual(
			valid,
		);
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(invalid)),
		).rejects.toBeDefined();
	});

	it.each(["approval_id", "mutation_id"] as const)(
		"requires resolution binding field %s",
		async (field) => {
			const payload = {
				approval_id: "approval_1",
				approved: true,
				mutation_id: "mutation_1",
			};
			delete payload[field];

			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope(
						attributed_mutation_envelope("git.mutation.resolve", payload),
					),
				),
			).rejects.toBeDefined();
		},
	);

	it.each(["A".repeat(64), "a".repeat(63), "a".repeat(65)])(
		"rejects malformed snapshot identifier %j",
		async (malformed_snapshot_id) => {
			const envelope = attributed_mutation_envelope("git.index.stage.request", {
				...mutation_request(),
				expected_snapshot_id: malformed_snapshot_id,
			});

			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		},
	);

	it.each(["bad..branch", "-option", "feature//codec", "feature.lock"])(
		"rejects invalid branch name %j",
		async (name) => {
			await expect(
				Effect.runPromise(
					Schema.decodeUnknownEffect(GitWorkspaceProjection, {
						onExcessProperty: "error",
					})({ ...repository(), branch: { name, type: "attached" } }),
				),
			).rejects.toBeDefined();
		},
	);

	it("rejects status flags that disagree with the porcelain status", async () => {
		const workspace = repository();
		workspace.files[0]!.flags.staged = false;

		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(GitWorkspaceProjection, {
					onExcessProperty: "error",
				})(workspace),
			),
		).rejects.toBeDefined();
	});

	it("rejects lifecycle metadata that could falsely claim success", async () => {
		const invalid: Record<string, unknown> = { ...mutation("succeeded") };
		delete invalid.result_workspace_version;

		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(GitMutationProjection, {
					onExcessProperty: "error",
				})(invalid),
			),
		).rejects.toBeDefined();
	});

	it("rejects patch bytes in durable workspace events", async () => {
		const workspace = { ...repository(), patch: "private diff" };
		const envelope = event({ cause: "refresh", type: "git.workspace.updated", workspace }, 7);

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects mismatched and oversized ephemeral diff patches", async () => {
		const mismatch = backend_envelope("git.diff.query.result", {
			byte_count: 5,
			format: "unified",
			format_version: 1,
			patch: "diff",
			scope: "aggregate",
			snapshot_id,
			truncated: false,
			workspace_id: "workspace_1",
			workspace_version: 3,
		});
		const oversized_patch = "x".repeat(git_diff_maximum_bytes + 1);
		const oversized = backend_envelope("git.diff.query.result", {
			byte_count: oversized_patch.length,
			format: "unified",
			format_version: 1,
			patch: oversized_patch,
			scope: "aggregate",
			snapshot_id,
			truncated: true,
			workspace_id: "workspace_1",
			workspace_version: 3,
		});

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(mismatch)),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(oversized)),
		).rejects.toBeDefined();
	});

	it("rejects excess Git request properties", async () => {
		const envelope = attributed_mutation_envelope("git.index.stage.request", {
			...mutation_request(),
			checkout: "main",
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});
});
