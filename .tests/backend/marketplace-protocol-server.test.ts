import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	HelloEnvelope,
	InboundControlEnvelope,
	OutboundControlEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	CapabilityTransportRegistry,
	NpxSkillsAdapter,
	ProtocolServer,
	RoutineInstaller,
	RoutineMirrorRegistry,
	RoutineSourceInspector,
} from "@artisan/backend";

import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "backend_marketplace_protocol",
	MakeId: (prefix) => Effect.succeed(`${prefix}_marketplace_protocol`),
	Now: Effect.succeed("2026-07-18T20:00:00.000Z"),
});

const hello: HelloEnvelope = {
	kind: "hello",
	message_id: "hello_marketplace",
	origin: "frontend",
	payload: { event_cursors: [], last_journal_sequence: 0, supported_protocol_versions: [1] },
	schema_version: 1,
	sent_at: "2026-07-18T20:00:00.000Z",
};

const trace = (message_id: string) => ({
	message_id,
	origin: "frontend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-07-18T20:00:00.000Z",
});

async function database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-marketplace-protocol-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

const receive_until = (
	connection: { readonly Outbound: Stream.Stream<OutboundControlEnvelope> },
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) => connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("Marketplace protocol server", () => {
	it("acquires the production Layer without discovering, installing, connecting, or synchronizing", async () => {
		const calls = { discover: 0, install: 0, inspect: 0, rollback: 0, sync: 0 };
		const runtime = make_backend_runtime({
			database_path: await database_path(),
			migrations_path,
			runtime_metadata: MetadataLive,
			npx_skills_adapter: Layer.succeed(NpxSkillsAdapter, {
				Discover: () =>
					Effect.sync(() => {
						calls.discover += 1;
						return { candidates: [], package_spec: "unused" };
					}),
			}),
			routine_installer: Layer.succeed(RoutineInstaller, {
				Install: () =>
					Effect.sync(() => {
						calls.install += 1;
						return { artifact_refs: [] };
					}),
				Rollback: () =>
					Effect.sync(() => {
						calls.rollback += 1;
					}),
			}),
			routine_mirror_registry: Layer.succeed(RoutineMirrorRegistry, {
				Find: () => {
					calls.sync += 1;
					return undefined;
				},
			}),
			routine_source_inspector: Layer.succeed(RoutineSourceInspector, {
				Inspect: () =>
					Effect.sync(() => {
						calls.inspect += 1;
						throw new Error("not called");
					}),
			}),
		});
		try {
			await runtime.runPromise(Effect.service(ProtocolServer));
			expect(calls).toEqual({ discover: 0, install: 0, inspect: 0, rollback: 0, sync: 0 });
		} finally {
			await runtime.dispose();
		}
	});

	it("correlates preview and receipts while denial performs no installer action", async () => {
		let installs = 0;
		const runtime = make_backend_runtime({
			database_path: await database_path(),
			migrations_path,
			runtime_metadata: MetadataLive,
			routine_installer: Layer.succeed(RoutineInstaller, {
				Install: () =>
					Effect.sync(() => {
						installs += 1;
						return { artifact_refs: [] };
					}),
				Rollback: () => Effect.void,
			}),
			routine_source_inspector: Layer.succeed(RoutineSourceInspector, {
				Inspect: ({ scope, source }) =>
					Effect.succeed({
						artifact_refs: [],
						candidate_id: "routine_previewed",
						compatibility: [],
						content_hashes: { "SKILL.md": "hash" },
						description: "Previewed routine",
						display_name: "Previewed",
						exported_commands: [],
						files: [{ path: "SKILL.md", required: true }],
						instructions: "Instructions",
						permissions: [],
						rollback_available: true,
						scope,
						source,
						trust: "known",
						version: "1.0.0",
					}),
			}),
		});
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						let connection = yield* server.Open;
						yield* connection.Receive(hello);
						yield* receive_until(
							connection,
							(envelope) => envelope.kind === "replay.complete",
						);
						const preview_request = {
							...trace("preview_request"),
							kind: "marketplace.routine.install.preview",
							payload: {
								scope: { kind: "global" },
								source: { kind: "catalog", locator: "catalog://previewed" },
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(preview_request);
						const preview_output = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "marketplace.routine.install.preview.result",
						);
						const preview = [...preview_output].find(
							(envelope) =>
								envelope.kind === "marketplace.routine.install.preview.result",
						);
						if (preview?.kind !== "marketplace.routine.install.preview.result")
							return yield* Effect.die("preview missing");
						const install_request = {
							...trace("install_request"),
							kind: "marketplace.routine.install.request",
							payload: {
								approval_id: "approval_previewed",
								preview_fingerprint: preview.payload.preview_fingerprint,
								requested_by: "user",
								scope: { kind: "global" },
								source: { kind: "catalog", locator: "catalog://previewed" },
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(install_request);
						const request_output = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "command.receipt" &&
								envelope.correlation_id === "install_request",
						);
						yield* connection.Close;
						connection = yield* server.Open;
						yield* connection.Receive({
							...hello,
							message_id: "hello_marketplace_decision",
						});
						yield* receive_until(
							connection,
							(envelope) => envelope.kind === "replay.complete",
						);
						const install_denied = {
							...trace("install_denied"),
							kind: "marketplace.routine.install.decision",
							payload: {
								approval_id: "approval_previewed",
								approved: false,
								preview_fingerprint: preview.payload.preview_fingerprint,
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(install_denied);
						const decision_output = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "command.receipt" &&
								envelope.correlation_id === "install_denied",
						);
						return {
							decision_output: [...decision_output],
							preview,
							request_output: [...request_output],
						};
					}),
				),
			);
			expect(result.preview.correlation_id).toBe("preview_request");
			expect(
				result.request_output.some(
					(envelope) =>
						envelope.kind === "command.receipt" &&
						envelope.payload.status === "accepted",
				),
			).toBe(true);
			expect(
				result.decision_output.some(
					(envelope) =>
						envelope.kind === "command.receipt" &&
						envelope.payload.status === "rejected",
				),
			).toBe(true);
			expect(installs).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects capability lifecycle actions without a durable approved claim", async () => {
		let connections = 0;
		const runtime = make_backend_runtime({
			capability_transport_registry: Layer.succeed(CapabilityTransportRegistry, {
				Connect: () =>
					Effect.sync(() => {
						connections += 1;
						return {
							CallTool: () => Effect.succeed({}),
							Close: Effect.void,
							Health: Effect.succeed("connected" as const),
							Initialize: Effect.succeed({
								protocol_version: "2025-06-18",
								server_name: "fake",
							}),
							ListResources: Effect.succeed([]),
							ListTools: Effect.succeed([]),
						};
					}),
			}),
			database_path: await database_path(),
			migrations_path,
			runtime_metadata: MetadataLive,
		});
		try {
			const receipts = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						let connection = yield* server.Open;
						yield* connection.Receive(hello);
						yield* receive_until(
							connection,
							(envelope) => envelope.kind === "replay.complete",
						);
						const preview_request = {
							...trace("capability_preview"),
							kind: "marketplace.capability.connect.preview",
							payload: {
								auth: { kind: "none" },
								scope: { kind: "global" },
								source: { kind: "catalog", locator: "https://example.invalid/mcp" },
								transport: {
									kind: "streamable_http",
									url: "https://example.invalid/mcp",
								},
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(preview_request);
						const preview_output = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "marketplace.capability.connect.preview.result",
						);
						const preview = [...preview_output].find(
							(envelope) =>
								envelope.kind === "marketplace.capability.connect.preview.result",
						);
						if (preview?.kind !== "marketplace.capability.connect.preview.result")
							return yield* Effect.die("capability preview missing");
						const request = {
							...trace("capability_request"),
							kind: "marketplace.capability.connect.request",
							payload: {
								approval_id: "approval_capability",
								auth: { kind: "none" },
								preview_fingerprint: preview.payload.preview_fingerprint,
								requested_by: "user",
								scope: { kind: "global" },
								source: { kind: "catalog", locator: "https://example.invalid/mcp" },
								transport: {
									kind: "streamable_http",
									url: "https://example.invalid/mcp",
								},
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(request);
						const requested = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "command.receipt" &&
								envelope.correlation_id === "capability_request",
						);
						yield* connection.Close;
						connection = yield* server.Open;
						yield* connection.Receive({
							...hello,
							message_id: "hello_capability_decision",
						});
						yield* receive_until(
							connection,
							(envelope) => envelope.kind === "replay.complete",
						);
						const denied = {
							...trace("capability_denied"),
							kind: "marketplace.capability.connect.decision",
							payload: {
								approval_id: "approval_capability",
								approved: false,
								preview_fingerprint: preview.payload.preview_fingerprint,
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(denied);
						const decided = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "command.receipt" &&
								envelope.correlation_id === "capability_denied",
						);
						const start = {
							...trace("capability_start"),
							kind: "marketplace.capability.start",
							payload: {
								capability_id: preview.payload.candidate_id,
								scope: { kind: "global" },
							},
						} satisfies InboundControlEnvelope;
						yield* connection.Receive(start);
						const started = yield* receive_until(
							connection,
							(envelope) =>
								envelope.kind === "command.receipt" &&
								envelope.correlation_id === "capability_start",
						);
						return {
							decided: [...decided],
							requested: [...requested],
							started: [...started],
						};
					}),
				),
			);
			expect(
				receipts.requested.some(
					(envelope) =>
						envelope.kind === "command.receipt" &&
						envelope.payload.status === "accepted",
				),
			).toBe(true);
			expect(
				receipts.decided.some(
					(envelope) =>
						envelope.kind === "command.receipt" &&
						envelope.payload.status === "rejected",
				),
			).toBe(true);
			expect(
				receipts.started.some(
					(envelope) =>
						envelope.kind === "command.receipt" &&
						envelope.payload.status === "rejected",
				),
			).toBe(true);
			expect(connections).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});
});
