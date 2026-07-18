import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Fiber, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer } from "@artisan/backend";
import { ArtisanClientError } from "@artisan/transport/client";

import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const policy = {
	approval: "always" as const,
	allow_engine_observation: true,
	allow_git_index_write: true,
	allow_preview_control: true,
	allow_process_control: true,
	allow_workspace_read: true,
	allow_workspace_write: true,
};

afterEach(async () =>
	Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	),
);

describe("ArtisanClient built-in tools with the backend ProtocolServer", () => {
	it("exposes registry, execution, approvals, history, events, and truthful workspace failures", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-client-tool-protocol-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		const thread_id = "thread_tool_transport";

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const observed_events = yield* harness.client.Events.pipe(
							Stream.filter((event) =>
								[
									"artisan.approval.updated",
									"artisan.assumption.recorded",
									"artisan.tool.invocation.updated",
								].includes(event.payload.type),
							),
							Stream.takeUntil(
								(event) => event.payload.type === "artisan.approval.updated",
							),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* harness.client.Command({
							command_id: "create_tool_transport_thread",
							payload: { title: "Tool transport", type: "thread.create" },
							thread_id,
						});
						const registry = yield* harness.client.ListArtisanTools({ policy });
						const discovery_error = yield* harness.client
							.ListWorkspaceFiles({ workspace_id: "missing_workspace" })
							.pipe(Effect.flip);
						const language_error = yield* harness.client
							.GetWorkspaceLanguageCapabilities({ workspace_id: "missing_workspace" })
							.pipe(Effect.flip);

						const assumption_receipt = yield* harness.client.ExecuteArtisanTool({
							command_id: "execute_assumption",
							input: {
								assumption_id: "assumption_transport",
								statement: "Use the existing transport contract.",
								tool_id: "assumption.record",
							},
							invocation_id: "invocation_assumption_transport",
							policy,
							thread_id,
						});
						const approval_receipt = yield* harness.client.ExecuteArtisanTool({
							command_id: "execute_approval",
							input: {
								approval_id: "approval_transport",
								description: "Confirm the transport approval.",
								permission_requirements: ["user_interaction"],
								tool_id: "approval.request",
							},
							invocation_id: "invocation_approval_transport",
							policy: { ...policy, approval: "never" },
							thread_id,
						});
						const resolution_receipt = yield* harness.client.ResolveArtisanApproval({
							approval_id: "approval_transport",
							approved: true,
							command_id: "resolve_approval",
							invocation_id: "invocation_approval_transport",
							resolution_id: "resolution_transport",
							thread_id,
						});
						const invocations = yield* harness.client.ListArtisanToolInvocations({
							thread_id,
						});
						const approvals = yield* harness.client.ListArtisanApprovals({ thread_id });
						const events = yield* Fiber.join(observed_events).pipe(
							Effect.timeout("5 seconds"),
						);

						return {
							approvals,
							discovery_error,
							events,
							invocations,
							language_error,
							receipts: [assumption_receipt, approval_receipt, resolution_receipt],
							registry,
						};
					}),
				),
			);

			expect(result.registry.declarations.map((entry) => entry.descriptor.id)).toEqual(
				expect.arrayContaining(["assumption.record", "approval.request"]),
			);
			expect(result.registry.availability).toEqual(
				expect.arrayContaining([
					expect.objectContaining({ state: "available", tool_id: "assumption.record" }),
				]),
			);
			expect(result.discovery_error).toBeInstanceOf(ArtisanClientError);
			expect(result.discovery_error).toMatchObject({
				code: "protocol",
				protocol_code: "workspace.discovery.unavailable",
			});
			expect(result.language_error).toBeInstanceOf(ArtisanClientError);
			expect(result.language_error).toMatchObject({
				code: "protocol",
				protocol_code: "workspace.language.unavailable",
			});
			expect(result.receipts).toEqual([
				expect.objectContaining({ command_id: "execute_assumption", status: "accepted" }),
				expect.objectContaining({ command_id: "execute_approval", status: "accepted" }),
				expect.objectContaining({ command_id: "resolve_approval", status: "accepted" }),
			]);
			expect(result.invocations.invocations).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						invocation_id: "invocation_assumption_transport",
						lifecycle: "succeeded",
					}),
					expect.objectContaining({
						invocation_id: "invocation_approval_transport",
						lifecycle: "succeeded",
					}),
				]),
			);
			expect(result.approvals.approvals).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						request: expect.objectContaining({ approval_id: "approval_transport" }),
						state: "resolved",
					}),
				]),
			);
			expect(Array.from(result.events, (event) => event.payload.type)).toEqual(
				expect.arrayContaining([
					"artisan.approval.updated",
					"artisan.assumption.recorded",
					"artisan.tool.invocation.updated",
				]),
			);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
