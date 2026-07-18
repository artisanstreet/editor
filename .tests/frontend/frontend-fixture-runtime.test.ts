import { createHash } from "node:crypto";
import { globSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, layer } from "@effect/vitest";
import { Context, Effect, Layer, Schema, Stream } from "effect";

import {
	GlobalGuidanceSnapshot,
	GitWorkspaceQueryResult,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	WorkspaceChange,
	WorkspaceFileReadQueryResult,
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

class FixtureRuntimeSources extends Context.Service<
	FixtureRuntimeSources,
	{
		readonly fixture_source: string;
		readonly production_sources: ReadonlyArray<string>;
	}
>()("Artisan/Test/FixtureRuntimeSources") {}

const FixtureRuntimeSourcesLive = Layer.effect(
	FixtureRuntimeSources,
	Effect.gen(function* () {
		const fixture_source = yield* Effect.try(() => readFileSync(fixture_source_path, "utf8"));
		const production_paths = yield* Effect.try(() =>
			globSync("modules/frontend/src/**/*.{ts,sv}", {
				exclude: ["modules/frontend/src/lib/runtime/fixtures/**"],
			}),
		);
		const production_sources: Array<string> = [];

		for (const path of production_paths) {
			production_sources.push(yield* Effect.try(() => readFileSync(path, "utf8")));
		}

		return FixtureRuntimeSources.of({ fixture_source, production_sources });
	}),
);

const FixtureServiceContract: typeof ArtisanClient.Service = FixtureArtisanClientService;
const FixtureLayerContract: Layer.Layer<ArtisanClient> = FixtureArtisanClientLayer;

describe("frontend ArtisanClient fixture runtime", () => {
	layer(FixtureRuntimeSourcesLive)((it) => {
		it.effect("satisfies the complete public client service shape", () =>
			Effect.gen(function* () {
				yield* Effect.void;

				expect(Object.keys(FixtureServiceContract).sort()).toEqual(
					[
						"Command",
						"Cursors",
						"Dispose",
						"Errors",
						"Events",
						"GetGlobalGuidance",
						"GetGitDiff",
						"GetGitWorkspace",
						"GetModelBehaviour",
						"GetPreviewAssetMetadata",
						"GetPreviewTarget",
						"GetOrchestrationGraph",
						"GetThreadTranscript",
						"GetThreadRetentionPolicy",
						"GetThreadWork",
						"GetWorkspaceChangeDiff",
						"GetWorkspaceLanguageCapabilities",
						"ExecuteArtisanTool",
						"ListArtisanApprovals",
						"ListArtisanToolInvocations",
						"ListArtisanTools",
						"ListTerminals",
						"ListOrchestrationGroups",
						"ListThreads",
						"ListPreviewTargets",
						"ListWorkspaceChanges",
						"ListWorkspaceFiles",
						"OpenAsset",
						"LaunchPreviewInExternalBrowser",
						"OpenPreviewInspectionSession",
						"InspectPreviewSession",
						"ClosePreviewInspectionSession",
						"OpenTerminalOutput",
						"ReadWorkspaceFile",
						"ReplaceWorkspaceFile",
						"RequestGitIndexMutation",
						"ResolveGitMutation",
						"ResolveArtisanApproval",
						"ResolveGlobalGuidanceDrift",
						"ResolveModelBehaviourDrift",
						"ResolveRichLink",
						"RetryGlobalGuidanceSync",
						"RetryModelBehaviourSync",
						"ProbePreviewTarget",
						"RegisterPreviewTarget",
						"RemovePreviewTarget",
						"ReviewWorkspaceChange",
						"RollbackWorkspaceChange",
						"SelectGlobalGuidance",
						"SetPreviewTargetState",
						"SubscribeOrchestrationGraph",
						"SubscribeOrchestrationGroups",
						"SubscribeThreadList",
						"SubscribeThreadTranscript",
						"UpdateGlobalGuidance",
						"UpdateModelBehaviour",
						"UpdateThreadRetentionPolicy",
					].sort(),
				);
				expect(FixtureLayerContract).toBeDefined();
			}),
		);

		it.effect("carries deterministic protocol-shaped visual data", () =>
			Effect.gen(function* () {
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
				expect(fixture_artisan_client_data.workspace_changes).toHaveLength(1);

				const guidance_content = fixture_artisan_client_data.global_guidance.content;
				const guidance_hash = createHash("sha256").update(guidance_content).digest("hex");
				expect(Buffer.byteLength(guidance_content)).toBe(
					fixture_artisan_client_data.global_guidance.metadata.canonical.byte_count,
				);
				expect(guidance_hash).toBe(
					fixture_artisan_client_data.global_guidance.metadata.canonical.content_hash,
				);

				yield* Schema.decodeUnknownEffect(GlobalGuidanceSnapshot)(
					fixture_artisan_client_data.global_guidance,
				);
				yield* Schema.decodeUnknownEffect(GitWorkspaceQueryResult)(
					fixture_artisan_client_data.git_workspace,
				);
				yield* Schema.decodeUnknownEffect(ModelBehaviourSnapshot)(
					fixture_artisan_client_data.model_behaviour,
				);
				yield* Schema.decodeUnknownEffect(OrchestrationGraph)(
					fixture_artisan_client_data.orchestration_graph,
				);
				yield* Schema.decodeUnknownEffect(Schema.Array(TerminalSession))(
					fixture_artisan_client_data.terminals,
				);
				yield* Schema.decodeUnknownEffect(Schema.Array(ThreadListItem))(
					fixture_artisan_client_data.threads,
				);
				yield* Schema.decodeUnknownEffect(ThreadRetentionPolicy)(
					fixture_artisan_client_data.thread_retention_policy,
				);
				yield* Schema.decodeUnknownEffect(ThreadWorkItem)(
					fixture_artisan_client_data.thread_work,
				);
				yield* Schema.decodeUnknownEffect(Schema.Array(WorkspaceChange))(
					fixture_artisan_client_data.workspace_changes,
				);

				for (const file of Object.values(fixture_artisan_client_data.workspace_files)) {
					yield* Schema.decodeUnknownEffect(WorkspaceFileReadQueryResult)(file);
				}
			}),
		);

		it.effect("uses an explicit Layer.succeed fixture with Effect-native behavior", () =>
			Effect.gen(function* () {
				const { fixture_source } = yield* FixtureRuntimeSources;

				expect(fixture_source).toContain(
					"Layer.succeed(ArtisanClient, FixtureArtisanClientService)",
				);
				expect(fixture_source).not.toMatch(/Effect\.run[A-Z]/);
				expect(fixture_source.match(/Effect\.gen\(/g)?.length ?? 0).toBeGreaterThanOrEqual(
					25,
				);
				expect(fixture_source).not.toContain("make_artisan_client_layer");
			}),
		);

		it.effect("is not silently selected by production frontend source", () =>
			Effect.gen(function* () {
				const { production_sources } = yield* FixtureRuntimeSources;
				const aggregate = production_sources.join("\n");

				expect(aggregate).not.toContain("FixtureArtisanClientLayer");
				expect(aggregate).not.toContain("artisan-client-fixture");
			}),
		);
	});

	layer(FixtureArtisanClientLayer)((it) => {
		it.effect("honors transcript pagination and orchestration terminal filters", () =>
			Effect.gen(function* () {
				const client = yield* ArtisanClient;
				const before = yield* client.GetThreadTranscript({
					before_journal_sequence: 47,
					limit: 1,
					thread_id: "thread-editor-shell",
				});
				const after = yield* client.GetThreadTranscript({
					after_journal_sequence: 46,
					limit: 1,
					thread_id: "thread-editor-shell",
				});
				const active = yield* client.ListOrchestrationGroups(
					"thread-editor-shell",
					false,
				);
				const all = yield* client.ListOrchestrationGroups("thread-editor-shell", true);
				const subscribed = yield* client.SubscribeOrchestrationGroups(
					"thread-editor-shell",
					false,
				);
				const updates = [...(yield* Stream.runCollect(subscribed))];

				expect(before).toMatchObject({ status: "available", entries: [] });
				expect(after).toMatchObject({
					status: "available",
					entries: [{ journal_sequence: 47 }],
				});
				expect(active.groups.map((group) => group.state)).toEqual(["running"]);
				expect(all.groups.map((group) => group.state)).toEqual(["running", "complete"]);
				expect(updates[0]).toMatchObject({
					type: "snapshot",
					snapshot: { groups: [{ state: "running" }] },
				});
			}),
		);

		it.effect("exposes deterministic workspace file and change operations", () =>
			Effect.gen(function* () {
				const client = yield* ArtisanClient;
				const listed = yield* client.ListWorkspaceChanges({
					thread_id: "thread-editor-shell",
					workspace_id: "workspace-artisan-editor",
				});
				const file = yield* client.ReadWorkspaceFile({
					path: "modules/frontend/src/lib/fixture.ts",
					workspace_id: "workspace-artisan-editor",
				});

				expect(listed.changes).toHaveLength(1);
				expect(file.content).toBe("export const fixture = true;\n");
				expect(
					yield* client.ReplaceWorkspaceFile({
						agent_id: "agent-terra",
						change_id: "change-fixture-runtime-2",
						command_id: "command-replace-fixture",
						content: "export const fixture = false;\n",
						expected_before: file.identity,
						path: file.path,
						run_id: "run-editor-shell",
						thread_id: "thread-editor-shell",
						workspace_id: file.workspace_id,
					}),
				).toMatchObject({ command_id: "command-replace-fixture", status: "accepted" });
				expect(
					yield* client.ReviewWorkspaceChange({
						change_id: listed.changes[0]!.change_id,
						command_id: "command-review-fixture",
						thread_id: "thread-editor-shell",
					}),
				).toMatchObject({ command_id: "command-review-fixture", status: "accepted" });
				expect(
					yield* client.RollbackWorkspaceChange({
						change_id: listed.changes[0]!.change_id,
						command_id: "command-rollback-fixture",
						expected_after: listed.changes[0]!.after_identity,
						thread_id: "thread-editor-shell",
					}),
				).toMatchObject({ command_id: "command-rollback-fixture", status: "accepted" });
			}),
		);
	});
});
