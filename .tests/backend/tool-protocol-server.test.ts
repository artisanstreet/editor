import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer, type ProtocolConnection } from "@artisan/backend";
import type { OutboundControlEnvelope } from "@artisan/protocol";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];
const policy = {
	approval: "always" as const,
	allow_engine_observation: true,
	allow_git_index_write: true,
	allow_preview_control: true,
	allow_process_control: true,
	allow_workspace_read: true,
	allow_workspace_write: true,
};

const envelope = (kind: string, message_id: string, payload: unknown, thread_id?: string) => ({
	kind,
	message_id,
	origin: "frontend" as const,
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-18T12:00:00.000Z",
	...(thread_id === undefined ? {} : { thread_id }),
});

const read_until = (connection: ProtocolConnection, kind: string) =>
	connection.Outbound.pipe(
		Stream.takeUntil((frame) => frame.kind === kind),
		Stream.runCollect,
	);

const negotiate = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		yield* connection.Receive({
			kind: "hello",
			message_id: "hello",
			origin: "frontend",
			payload: {
				event_cursors: [],
				last_journal_sequence: 0,
				supported_protocol_versions: [1],
			},
			schema_version: 1,
			sent_at: "2026-07-18T12:00:00.000Z",
		});
		yield* read_until(connection, "replay.complete");
	});

afterEach(async () =>
	Promise.all(directories.splice(0).map((path) => rm(path, { force: true, recursive: true }))),
);

describe("built-in tool protocol routes", () => {
	it("returns renderer-safe registry, invocation, approval, discovery, language, execute, and resolve frames", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-tool-protocol-"));
		directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});
		try {
			const frames = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* negotiate(connection);
						yield* connection.Receive(
							envelope(
								"command",
								"create",
								{ title: "Tools", type: "thread.create" },
								"thread_tools",
							),
						);
						yield* read_until(connection, "command.receipt");
						const queries = [
							envelope("artisan.tool.registry.list.query", "registry", {
								policy,
								thread_id: "thread_tools",
							}),
							envelope("artisan.tool.invocation.list.query", "invocations", {
								thread_id: "thread_tools",
							}),
							envelope("artisan.approval.list.query", "approvals", {
								thread_id: "thread_tools",
							}),
							envelope("workspace.file.discovery.query", "discovery", {
								workspace_id: "missing",
							}),
							envelope("workspace.language.capabilities.query", "language", {
								workspace_id: "missing",
							}),
						] as const;
						const results: OutboundControlEnvelope[] = [];
						for (const query of queries) {
							yield* connection.Receive(query);
							results.push(
								...(yield* read_until(
									connection,
									query.kind.includes("discovery")
										? "protocol.error"
										: query.kind.includes("language")
											? "protocol.error"
											: `${query.kind}.result`,
								)),
							);
						}
						yield* connection.Receive(
							envelope(
								"artisan.tool.execute",
								"execute",
								{
									invocation_id: "invocation",
									policy,
									input: {
										tool_id: "assumption.record",
										assumption_id: "assumption",
										statement: "safe",
									},
								},
								"thread_tools",
							),
						);
						results.push(...(yield* read_until(connection, "command.receipt")));
						yield* connection.Receive(
							envelope(
								"artisan.tool.execute",
								"approval_execute",
								{
									invocation_id: "approval_invocation",
									policy: { ...policy, approval: "never" },
									input: {
										tool_id: "approval.request",
										approval_id: "approval_known",
										description: "Confirm",
										permission_requirements: ["user_interaction"],
									},
								},
								"thread_tools",
							),
						);
						results.push(...(yield* read_until(connection, "command.receipt")));
						yield* connection.Receive(
							envelope(
								"artisan.approval.resolve",
								"resolve",
								{
									approval_id: "approval_known",
									approved: true,
									invocation_id: "approval_invocation",
									resolution_id: "resolution",
								},
								"thread_tools",
							),
						);
						results.push(...(yield* read_until(connection, "command.receipt")));
						for (const query of [
							envelope("artisan.tool.invocation.list.query", "invocations_after", {
								thread_id: "thread_tools",
							}),
							envelope("artisan.approval.list.query", "approvals_after", {
								thread_id: "thread_tools",
							}),
						]) {
							yield* connection.Receive(query);
							results.push(
								...(yield* read_until(connection, `${query.kind}.result`)),
							);
						}
						return results;
					}),
				),
			);
			expect(
				frames.some((frame) => frame.kind === "artisan.tool.registry.list.query.result"),
			).toBe(true);
			expect(
				frames.some((frame) => frame.kind === "artisan.tool.invocation.list.query.result"),
			).toBe(true);
			expect(
				frames.some((frame) => frame.kind === "artisan.approval.list.query.result"),
			).toBe(true);
			expect(frames.filter((frame) => frame.kind === "protocol.error")).toHaveLength(2);
			expect(
				frames.filter((frame) => frame.kind === "command.receipt").length,
			).toBeGreaterThanOrEqual(3);
			const invocation_result = frames
				.filter((frame) => frame.kind === "artisan.tool.invocation.list.query.result")
				.at(-1);
			const approval_result = frames
				.filter((frame) => frame.kind === "artisan.approval.list.query.result")
				.at(-1);
			expect(invocation_result?.payload).toMatchObject({
				invocations: expect.arrayContaining([
					expect.objectContaining({
						invocation_id: "approval_invocation",
						lifecycle: "succeeded",
					}),
				]),
			});
			expect(approval_result?.payload).toMatchObject({
				approvals: expect.arrayContaining([
					expect.objectContaining({
						request: expect.objectContaining({ approval_id: "approval_known" }),
						state: "resolved",
					}),
				]),
			});
		} finally {
			await runtime.dispose();
		}
	});
});
