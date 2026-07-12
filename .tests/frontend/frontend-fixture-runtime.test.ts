import { join } from "node:path";
import { createHash } from "node:crypto";
import { globSync, readFileSync } from "node:fs";

import { Layer, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	GlobalGuidanceSnapshot,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
} from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import {
	FixtureArtisanClientLayer,
	FixtureArtisanClientService,
	fixture_artisan_client_data,
} from "../../modules/frontend/src/lib/runtime/fixtures/artisan-client-fixture";

const fixture_source_path = join(
	process.cwd(),
	"modules/frontend/src/lib/runtime/fixtures/artisan-client-fixture.ts",
);

const fixture_source = readFileSync(fixture_source_path, "utf8");

const FixtureServiceContract: typeof ArtisanClient.Service = FixtureArtisanClientService;
const FixtureLayerContract: Layer.Layer<ArtisanClient> = FixtureArtisanClientLayer;

describe("frontend ArtisanClient fixture runtime", () => {
	it("satisfies the complete public client service shape", () => {
		expect(Object.keys(FixtureServiceContract).sort()).toEqual(
			[
				"Command",
				"Cursors",
				"Dispose",
				"Errors",
				"Events",
				"GetGlobalGuidance",
				"GetModelBehaviour",
				"GetOrchestrationGraph",
				"GetThreadRetentionPolicy",
				"GetThreadWork",
				"ListTerminals",
				"ListThreads",
				"ListWorkspaceChanges",
				"OpenAsset",
				"OpenTerminalOutput",
				"ReadWorkspaceFile",
				"ReplaceWorkspaceFile",
				"ResolveGlobalGuidanceDrift",
				"ResolveModelBehaviourDrift",
				"RetryGlobalGuidanceSync",
				"RetryModelBehaviourSync",
				"ReviewWorkspaceChange",
				"RollbackWorkspaceChange",
				"SelectGlobalGuidance",
				"SubscribeOrchestrationGraph",
				"SubscribeThreadList",
				"UpdateGlobalGuidance",
				"UpdateModelBehaviour",
				"UpdateThreadRetentionPolicy",
			].sort(),
		);
		expect(FixtureLayerContract).toBeDefined();
	});

	it("carries deterministic protocol-shaped visual data", () => {
		expect(fixture_artisan_client_data).toMatchObject({
			cursors: { last_journal_sequence: 48 },
			orchestration_graph: {
				group: {
					group_id: "group-editor-shell",
					thread_id: "thread-editor-shell",
				},
			},
			thread_retention_policy: { enabled: true, inactivity_days: 7 },
		});
		expect(fixture_artisan_client_data.threads).toHaveLength(1);
		expect(fixture_artisan_client_data.terminals).toHaveLength(1);

		const guidance_content = fixture_artisan_client_data.global_guidance.content;
		const guidance_hash = createHash("sha256").update(guidance_content).digest("hex");
		expect(Buffer.byteLength(guidance_content)).toBe(
			fixture_artisan_client_data.global_guidance.metadata.canonical.byte_count,
		);
		expect(guidance_hash).toBe(
			fixture_artisan_client_data.global_guidance.metadata.canonical.content_hash,
		);

		expect(() =>
			Schema.decodeUnknownSync(GlobalGuidanceSnapshot)(
				fixture_artisan_client_data.global_guidance,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(ModelBehaviourSnapshot)(
				fixture_artisan_client_data.model_behaviour,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(OrchestrationGraph)(
				fixture_artisan_client_data.orchestration_graph,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(Schema.Array(TerminalSession))(
				fixture_artisan_client_data.terminals,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(Schema.Array(ThreadListItem))(
				fixture_artisan_client_data.threads,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(ThreadRetentionPolicy)(
				fixture_artisan_client_data.thread_retention_policy,
			),
		).not.toThrow();
		expect(() =>
			Schema.decodeUnknownSync(ThreadWorkItem)(fixture_artisan_client_data.thread_work),
		).not.toThrow();
	});

	it("uses an explicit Layer.succeed fixture with Effect-native behavior", () => {
		expect(fixture_source).toContain(
			"Layer.succeed(ArtisanClient, FixtureArtisanClientService)",
		);
		expect(fixture_source).not.toMatch(/Effect\.run[A-Z]/);
		expect(fixture_source.match(/Effect\.gen\(/g)?.length ?? 0).toBeGreaterThanOrEqual(20);
		expect(fixture_source).not.toContain("make_artisan_client_layer");
	});

	it("is not silently selected by production frontend source", () => {
		const production_sources = globSync("modules/frontend/src/**/*.{ts,sv}", {
			exclude: ["modules/frontend/src/lib/runtime/fixtures/**"],
		}).map((path) => readFileSync(path, "utf8"));

		expect(production_sources.join("\n")).not.toContain("FixtureArtisanClientLayer");
		expect(production_sources.join("\n")).not.toContain("artisan-client-fixture");
	});
});
