import { execFile as exec_file } from "node:child_process";
import { createHash } from "node:crypto";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Layer, Redacted, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	ContentIdentity,
	HelloEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";
import type { Engine, EngineOpenInput } from "@artisan/engines";
import {
	make_backend_runtime,
	make_workspace_bounded_regular_file_store_registry_layer,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";
import { make_fake_engine } from "../../engines/harness/fake-engine";

import { Database } from "../../../modules/backend/src/persistence/database";
import { OrchestrationRuns } from "../../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));
const RunCommand = promisify(exec_file);
const temporary_directories: Array<string> = [];
const sent_at = "2026-07-18T12:00:00.000Z";
const receipt_key = Redacted.make(new Uint8Array(32).fill(8));
const encoder = new TextEncoder();

type ReplacementOptions = {
	readonly expected: Uint8Array;
	readonly maximumBytes: number;
	readonly operationId: string;
	readonly path: string;
	readonly replacement: Uint8Array;
};

function ContentIdentityFor(content: string): ContentIdentity {
	const bytes = encoder.encode(content);

	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function SameBytes(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((value, index) => value === right[index])
	);
}

function MakeNativeModule(root: string, close_count: { value: number }) {
	const receipts = new Map<string, ReplacementOptions>();

	class FakeNativeBoundedRegularFileStore {
		constructor(
			readonly configured_root: string,
			_receipt_authentication_key: Uint8Array,
		) {}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(candidate_root === this.configured_root);
		}

		close() {
			close_count.value += 1;
		}

		async finalizeRegularFileReplacement(options: ReplacementOptions) {
			const receipt = receipts.get(options.operationId);

			if (receipt === undefined) {
				return;
			}

			if (
				receipt.path !== options.path ||
				!SameBytes(receipt.expected, options.expected) ||
				!SameBytes(receipt.replacement, options.replacement)
			) {
				throw new Error("replacement receipt intent changed");
			}

			receipts.delete(options.operationId);
		}

		async readRegularFile(path: string, maximum_bytes: number) {
			const bytes = new Uint8Array(await readFile(join(root, path)));

			if (bytes.byteLength > maximum_bytes) throw new Error("file exceeds maximum bytes");

			return bytes;
		}

		async replaceRegularFile(options: ReplacementOptions) {
			const receipt = receipts.get(options.operationId);

			if (receipt !== undefined) return "AlreadyReplaced";

			const target = join(root, options.path);
			const current = new Uint8Array(await readFile(target));

			if (
				!SameBytes(current, options.expected) ||
				options.replacement.byteLength > options.maximumBytes
			) {
				return "Changed";
			}

			await writeFile(target, options.replacement);
			receipts.set(options.operationId, {
				...options,
				expected: Uint8Array.from(options.expected),
				replacement: Uint8Array.from(options.replacement),
			});

			return "Replaced";
		}
	}

	return () => ({
		NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
		getNativeBuildDescriptor: () => ({
			architecture: "x86_64",
			operatingSystem: "windows",
			target: "x86_64-pc-windows-msvc",
			testHooksEnabled: false,
		}),
	});
}

async function MakeFixture() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-deep-workspace-"));
	const root = join(directory, "workspace");

	temporary_directories.push(directory);
	await mkdir(join(root, "src"), { recursive: true });
	await writeFile(join(root, "src", "example.ts"), "before\n");
	await RunCommand("git", ["init", "--initial-branch=main"], { cwd: root });
	await RunCommand("git", ["config", "user.email", "deep@example.test"], { cwd: root });
	await RunCommand("git", ["config", "user.name", "Deep Integration"], { cwd: root });
	await RunCommand("git", ["add", "."], { cwd: root });
	await RunCommand("git", ["-c", "commit.gpgsign=false", "commit", "-m", "fixture"], {
		cwd: root,
	});

	return { database_path: join(directory, "artisan.db"), directory, root };
}

function MakeRuntime(
	database_path: string,
	root: string,
	close_count: { value: number },
	instance_id: string,
	engine_cleanup_count: { value: number },
	engine?: Engine,
) {
	let next_id = 0;

	return make_backend_runtime({
		database_path,
		engines: [
			engine ??
				make_fake_engine({
					engine_id: "fake_engine",
					on_cleanup: () => {
						engine_cleanup_count.value += 1;
					},
				}),
		],
		migrations_path,
		runtime_metadata: Layer.succeed(RuntimeMetadata, {
			instance_id,
			MakeId: (prefix) => Effect.sync(() => `${instance_id}_${prefix}_${++next_id}`),
			Now: Effect.succeed(sent_at),
		}),
		workspace_bounded_regular_file_store_registry:
			make_workspace_bounded_regular_file_store_registry_layer(
				[{ root, workspace_id: "workspace_deep" }],
				{
					load_native_module: MakeNativeModule(root, close_count),
					receipt_authentication_key: receipt_key,
				},
			).pipe(Layer.provide(NodeFileSystem.layer)),
	});
}

/** Keeps the fixture's provider run observable until workspace mutation evidence is asserted. */
function MakeBlockingFakeEngine(engine_cleanup_count: { value: number }) {
	const release = Deferred.makeUnsafe<void>();
	const fake = make_fake_engine({
		engine_id: "fake_engine",
		on_cleanup: () => {
			engine_cleanup_count.value += 1;
		},
		transport: "test",
	});
	const engine = {
		...fake,
		Open: (input: EngineOpenInput) =>
			fake.Open(input).pipe(
				Effect.map((run) => ({
					...run,
					Closed: Deferred.await(release).pipe(Effect.as("closed" as const)),
					Events: Stream.concat(
						run.Events.pipe(Stream.filter((event) => event._tag !== "run_terminal")),
						Stream.fromEffect(Deferred.await(release)).pipe(Stream.drain),
					),
				})),
			),
	} satisfies Engine;

	return { engine, release };
}

const OpenConnection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

function TakeOutbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function TakeCorrelated(connection: ProtocolConnection, correlation_id: string, count: number) {
	return connection.Outbound.pipe(
		Stream.filter(
			(envelope) =>
				"correlation_id" in envelope && envelope.correlation_id === correlation_id,
		),
		Stream.take(count),
		Stream.runCollect,
	);
}

function TakeThroughReplayComplete(connection: ProtocolConnection) {
	return connection.Outbound.pipe(
		Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
		Stream.runCollect,
	);
}

function Hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_deep",
		origin: "frontend",
		payload: { event_cursors: [], last_journal_sequence: 0, supported_protocol_versions: [1] },
		schema_version: 1,
		sent_at,
	};
}

function Replace(agent_id: string, run_id: string): WorkspaceFileReplaceEnvelope {
	return {
		agent_id,
		kind: "workspace.file.replace",
		message_id: "replace_deep",
		origin: "frontend",
		payload: {
			change_id: "change_deep",
			content: "after\n",
			expected_before: ContentIdentityFor("before\n"),
			path: "src/example.ts",
			workspace_id: "workspace_deep",
		},
		protocol_version: 1,
		raw_origin: { provider: "codex", reference: "fake-engine-deep" },
		run_id,
		schema_version: 1,
		sent_at,
		thread_id: "thread_deep",
	};
}

function List(): WorkspaceChangeListQueryEnvelope {
	return {
		kind: "workspace.change.list.query",
		message_id: "list_deep",
		origin: "frontend",
		payload: { thread_id: "thread_deep", workspace_id: "workspace_deep" },
		protocol_version: 1,
		schema_version: 1,
		sent_at,
	};
}

function Diff(): WorkspaceChangeDiffQueryEnvelope {
	return {
		kind: "workspace.change.diff.query",
		message_id: "diff_deep",
		origin: "frontend",
		payload: { change_id: "change_deep", thread_id: "thread_deep" },
		protocol_version: 1,
		schema_version: 1,
		sent_at,
	};
}

function Command(thread_id: string, message_id: string, payload: Record<string, unknown>) {
	return {
		kind: "command" as const,
		message_id,
		origin: "frontend" as const,
		payload,
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at,
		thread_id,
	};
}

const RunningAuthority = (root: string) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const runs = yield* database.client.select().from(OrchestrationRuns);
		const run = runs.find(
			(candidate) =>
				candidate.thread_id === "thread_deep" && candidate.working_directory === root,
		);

		if (!run) {
			return yield* Effect.die(new Error("Public fake Engine start did not persist a run"));
		}

		return { agent_id: run.agent_id, run_id: run.run_id };
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("deep public-protocol workspace integration", () => {
	it("preserves one observable workspace effect and rebuilt projection across a backend restart", async () => {
		const { database_path, directory, root } = await MakeFixture();
		const first_close_count = { value: 0 };
		const first_engine_cleanup_count = { value: 0 };
		const blocking_engine = MakeBlockingFakeEngine(first_engine_cleanup_count);
		const first_runtime = MakeRuntime(
			database_path,
			root,
			first_close_count,
			"deep_first",
			first_engine_cleanup_count,
			blocking_engine.engine,
		);

		try {
			const output = await first_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* connection.Receive(Hello());
						yield* TakeThroughReplayComplete(connection);
						yield* connection.Receive(
							Command("thread_deep", "create_engine_thread", {
								title: "Deep integration",
								type: "thread.create",
							}),
						);
						yield* TakeCorrelated(connection, "create_engine_thread", 2);
						yield* connection.Receive(
							Command("thread_deep", "start_deep_run", {
								engine_id: "fake_engine",
								text: "Prepare the attributed workspace change",
								type: "thread.send_message",
								working_directory: root,
							}),
						);
						yield* TakeCorrelated(connection, "start_deep_run", 1);
						yield* connection.Outbound.pipe(
							Stream.filter(
								(envelope) =>
									envelope.kind === "event" &&
									envelope.payload.type === "run.lifecycle" &&
									envelope.payload.state === "running",
							),
							Stream.take(1),
							Stream.runDrain,
						);
						const authority = yield* RunningAuthority(root);
						const replacement = Replace(authority.agent_id, authority.run_id);

						yield* connection.Receive(replacement);
						const accepted = yield* TakeCorrelated(connection, "replace_deep", 2);
						yield* connection.Receive(replacement);
						const duplicate = yield* TakeCorrelated(connection, "replace_deep", 1);

						return { accepted, duplicate };
					}),
				),
			);
			expect(output.accepted).toMatchObject([
				{
					correlation_id: "replace_deep",
					kind: "command.receipt",
					payload: { status: "accepted" },
				},
				{
					correlation_id: "replace_deep",
					kind: "event",
					payload: { action: "recorded", type: "workspace.change.updated" },
				},
			]);
			expect(output.duplicate).toMatchObject([
				{
					correlation_id: "replace_deep",
					kind: "command.receipt",
					payload: { status: "duplicate" },
				},
			]);
			expect(await readFile(join(root, "src", "example.ts"), "utf8")).toBe("after\n");
			expect(
				(await RunCommand("git", ["diff", "--", "src/example.ts"], { cwd: root })).stdout,
			).toContain("+after");
			await Effect.runPromise(Deferred.succeed(blocking_engine.release, undefined));
		} finally {
			await first_runtime.dispose();
		}
		expect(first_engine_cleanup_count.value).toBe(1);

		const second_close_count = { value: 0 };
		const second_engine_cleanup_count = { value: 0 };
		const second_runtime = MakeRuntime(
			database_path,
			root,
			second_close_count,
			"deep_second",
			second_engine_cleanup_count,
		);

		try {
			const rebuilt = await second_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* connection.Receive(Hello());
						yield* TakeThroughReplayComplete(connection);
						yield* connection.Receive(List());
						const list = yield* TakeOutbound(connection, 1);
						yield* connection.Receive(Diff());
						const diff = yield* TakeOutbound(connection, 1);

						return { diff, list };
					}),
				),
			);

			expect(rebuilt.list).toMatchObject([
				{
					kind: "workspace.change.list.query.result",
					payload: {
						changes: [{ change_id: "change_deep", review_state: "needs_review" }],
					},
				},
			]);
			expect(rebuilt.diff).toMatchObject([
				{
					kind: "workspace.change.diff.query.result",
					payload: { change_id: "change_deep", patch: expect.stringContaining("+after") },
				},
			]);
		} finally {
			await second_runtime.dispose();
		}

		expect(first_close_count.value).toBeGreaterThan(0);
		expect(second_close_count.value).toBeGreaterThan(0);
		expect(second_engine_cleanup_count.value).toBe(0);
		await rm(directory, { force: true, recursive: true });
		temporary_directories.splice(temporary_directories.indexOf(directory), 1);
		await expect(access(directory)).rejects.toThrow();
	});
});
