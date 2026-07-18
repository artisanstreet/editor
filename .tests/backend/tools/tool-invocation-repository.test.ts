import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_database_layer, Database } from "../../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../../modules/backend/src/persistence/journal-notifier";
import { Threads } from "../../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/runtime-metadata";
import {
	ToolInvocationConflict,
	ToolInvocationRepository,
	ToolInvocationRepositoryLive,
} from "../../../modules/backend/src/tools/tool-invocation-repository";

const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const metadata = (instance_id: string) => {
	let id = 0;
	let time = 0;
	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++id}`),
		Now: Effect.sync(() => new Date(Date.UTC(2026, 6, 18, 12, 0, ++time)).toISOString()),
	});
};

const runtime = (database_path: string, instance_id: string) =>
	ManagedRuntime.make(
		ToolInvocationRepositoryLive.pipe(
			Layer.provideMerge(NodeCrypto.layer),
			Layer.provideMerge(
				Layer.mergeAll(
					make_database_layer({ database_path, migrations_path }),
					metadata(instance_id),
					JournalNotifierLive,
				),
			),
		),
	);

const invocation = (id = "invocation_1") => ({
	input_summary: "Stage src/index.ts",
	invocation_id: id,
	lifecycle: "requested" as const,
	permission: {
		decision: "allowed" as const,
		policy: {
			approval: "never" as const,
			allow_engine_observation: true,
			allow_git_index_write: true,
			allow_preview_control: true,
			allow_process_control: true,
			allow_workspace_read: true,
			allow_workspace_write: true,
		},
		requirements: ["git_index_write" as const],
		tool_id: "git.index.stage" as const,
	},
	requested_at: "2026-07-18T12:00:00.000Z",
	thread_id: "thread_1",
	tool_id: "git.index.stage" as const,
	updated_at: "2026-07-18T12:00:00.000Z",
});

const begin = (value = invocation()) => ({
	invocation: value,
	request_fingerprint: createHash("sha256")
		.update(JSON.stringify({ approval: null, invocation: value }))
		.digest("hex"),
});

afterEach(async () =>
	Promise.all(directories.splice(0).map((path) => rm(path, { force: true, recursive: true }))),
);

describe("ToolInvocationRepository", () => {
	it("persists a policy-denied initial invocation without an approval", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tool-denied-"));
		directories.push(directory);
		const live = runtime(join(directory, "artisan.db"), "denied");
		try {
			const result = await live.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client
						.insert(Threads)
						.values({
							created_at: "2026-07-18T12:00:00.000Z",
							thread_id: "thread_1",
							title: "thread",
							updated_at: "2026-07-18T12:00:00.000Z",
						});
					const repository = yield* ToolInvocationRepository;
					const denied = {
						...invocation(),
						completed_at: "2026-07-18T12:00:00.000Z",
						lifecycle: "denied" as const,
						outcome: { code: "permission_denied", status: "denied" as const },
						permission: { ...invocation().permission, decision: "denied" as const },
						updated_at: "2026-07-18T12:00:00.000Z",
					};
					const input = {
						invocation: denied,
						request_fingerprint: createHash("sha256")
							.update(JSON.stringify({ approval: null, invocation: denied }))
							.digest("hex"),
					};
					return yield* repository.Begin(input);
				}),
			);
			expect(result).toMatchObject({
				lifecycle: "denied",
				outcome: { code: "permission_denied" },
			});
		} finally {
			await live.dispose();
		}
	});

	it("resolves a bound approval into requested or durable denial", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tool-approval-"));
		directories.push(directory);
		const live = runtime(join(directory, "artisan.db"), "approval");
		try {
			await live.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-18T12:00:00.000Z",
						thread_id: "thread_1",
						title: "thread",
						updated_at: "2026-07-18T12:00:00.000Z",
					});
				}),
			);
			const result = await live.runPromise(
				Effect.gen(function* () {
					const repository = yield* ToolInvocationRepository;
					const pending = {
						...invocation(),
						approval_id: "approval_1",
						lifecycle: "awaiting_approval" as const,
					};
					const approval = {
						approval_id: "approval_1",
						description: "Stage files",
						invocation_id: "invocation_1",
						permission_requirements: ["git_index_write" as const],
						requested_at: pending.requested_at,
					};
					const input = {
						approval,
						invocation: pending,
						request_fingerprint: createHash("sha256")
							.update(JSON.stringify({ approval, invocation: pending }))
							.digest("hex"),
					};
					yield* repository.Begin(input);
					const resolved = yield* repository.ResolveApproval({
						approval_id: "approval_1",
						approved: false,
						invocation_id: "invocation_1",
						resolution_id: "resolution_1",
						resolved_at: "2026-07-18T12:01:00.000Z",
					});
					const [stored] = yield* repository.ListInvocations({ thread_id: "thread_1" });
					return { resolved, stored };
				}),
			);
			expect(result.resolved.state).toBe("resolved");
			expect(result.stored).toMatchObject({
				lifecycle: "denied",
				outcome: { code: "approval_denied", status: "denied" },
			});
		} finally {
			await live.dispose();
		}
	});

	it("persists exact begin retries, claims, finalization, list and usage across runtimes", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tool-invocation-"));
		directories.push(directory);
		const first = runtime(join(directory, "artisan.db"), "first");
		try {
			await first.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(Threads).values({
						created_at: "2026-07-18T12:00:00.000Z",
						thread_id: "thread_1",
						title: "thread",
						updated_at: "2026-07-18T12:00:00.000Z",
					});
				}),
			);
			const result = await first.runPromise(
				Effect.gen(function* () {
					const repository = yield* ToolInvocationRepository;
					const accepted = yield* repository.Begin(begin());
					const duplicate = yield* repository.Begin(begin());
					const claim = yield* repository.Claim("invocation_1");
					const terminal = yield* repository.Finalize("invocation_1", {
						code: "staged",
						status: "succeeded",
					});
					const replay = yield* repository.Finalize("invocation_1", {
						code: "staged",
						status: "succeeded",
					});
					const listed = yield* repository.ListInvocations({ thread_id: "thread_1" });
					const usage = yield* repository.Usage("thread_1");
					return { accepted, claim, duplicate, listed, replay, terminal, usage };
				}),
			);
			expect(result.accepted).toEqual(result.duplicate);
			expect(result.claim.status).toBe("claimed");
			expect(result.terminal).toEqual(result.replay);
			expect(result.listed).toHaveLength(1);
			expect(result.usage).toMatchObject([
				{
					active_invocation_count: 0,
					tool_id: "git.index.stage",
					total_invocation_count: 1,
				},
			]);
			await expect(
				first.runPromise(
					Effect.gen(function* () {
						const repository = yield* ToolInvocationRepository;
						return yield* repository.Begin({
							...begin(),
							request_fingerprint: "a".repeat(64),
						});
					}),
				),
			).rejects.toBeInstanceOf(ToolInvocationConflict);
		} finally {
			await first.dispose();
		}
	});
});
