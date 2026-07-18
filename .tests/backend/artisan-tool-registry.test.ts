import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanToolApprovalPolicyLive } from "../../modules/backend/src/tools/approval-policy";
import {
	ArtisanBuiltInToolRegistrations,
	ArtisanToolRegistry,
	make_artisan_tool_capability_state_layer,
	make_artisan_tool_registry_layer,
} from "../../modules/backend/src/tools/artisan-tool-registry";

const permissive_policy = {
	approval: "never" as const,
	allow_engine_observation: true,
	allow_git_index_write: true,
	allow_preview_control: true,
	allow_process_control: true,
	allow_workspace_read: true,
	allow_workspace_write: true,
};

function make_layer(
	states: ReadonlyArray<unknown> = ArtisanBuiltInToolRegistrations.map(({ tool_id }) => ({
		state: "available",
		tool_id,
	})),
	registrations: ReadonlyArray<unknown> = ArtisanBuiltInToolRegistrations,
) {
	return make_artisan_tool_registry_layer(registrations).pipe(
		Layer.provide(ArtisanToolApprovalPolicyLive),
		Layer.provide(make_artisan_tool_capability_state_layer(states)),
	);
}

describe("ArtisanToolRegistry", () => {
	it("exposes every canonical built-in declaration in deterministic order", async () => {
		const registry = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(Effect.provide(make_layer())),
		);

		expect(registry.Declarations.map((declaration) => declaration.descriptor.id)).toEqual([
			"approval.request",
			"assumption.record",
			"engine.native_action.record",
			"git.diff.read",
			"git.index.stage",
			"git.index.unstage",
			"git.status.read",
			"preview.inspect",
			"preview.open",
			"preview.stop",
			"question.ask",
			"terminal.open",
			"terminal.read",
			"terminal.restart",
			"terminal.stop",
			"terminal.write",
			"workspace.file.list",
			"workspace.file.read",
			"workspace.file.write",
			"workspace.language.status",
		]);
		expect(
			registry.Declarations.find(
				(declaration) => declaration.descriptor.id === "engine.native_action.record",
			)?.descriptor.permission_requirements,
		).toEqual(["engine_observation"]);
		expect(
			registry.Declarations.filter(
				(declaration) => declaration.descriptor.kind === "git",
			).map((declaration) => declaration.descriptor.id),
		).toEqual(["git.diff.read", "git.index.stage", "git.index.unstage", "git.status.read"]);
		expect(await Effect.runPromise(registry.Find("workspace.file.list"))).toMatchObject({
			descriptor: { id: "workspace.file.list" },
		});
	});

	it("combines runtime capability facts with policy decisions and fails closed for omitted capability state", async () => {
		const registry = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(
				Effect.provide(
					make_layer([
						{ state: "available", tool_id: "terminal.read" },
						{
							state: "unavailable",
							tool_id: "preview.open",
							unavailable_reason: "Preview adapter is not configured",
						},
					]),
				),
			),
		);
		const availability = await Effect.runPromise(
			registry.Availability({ policy: permissive_policy }),
		);

		expect(availability.find((entry) => entry.tool_id === "terminal.read")).toMatchObject({
			state: "available",
		});
		expect(availability.find((entry) => entry.tool_id === "preview.open")).toMatchObject({
			state: "unavailable",
			unavailable_reason: "Preview adapter is not configured",
		});
		expect(availability.find((entry) => entry.tool_id === "workspace.file.read")).toMatchObject(
			{ state: "unavailable", unavailable_reason: "Capability is not configured" },
		);
		expect(
			availability.find((entry) => entry.tool_id === "workspace.language.status"),
		).toMatchObject({
			state: "unavailable",
			unavailable_reason: "Capability is not configured",
		});
	});

	it("marks policy-sensitive tools as approval required or unavailable", async () => {
		const registry = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(Effect.provide(make_layer())),
		);
		const approval = await Effect.runPromise(
			registry.Availability({ policy: { ...permissive_policy, approval: "always" } }),
		);
		const denied = await Effect.runPromise(
			registry.Availability({
				policy: { ...permissive_policy, allow_workspace_write: false },
			}),
		);

		expect(approval.find((entry) => entry.tool_id === "workspace.file.write")).toMatchObject({
			state: "approval_required",
		});
		expect(denied.find((entry) => entry.tool_id === "workspace.file.write")).toMatchObject({
			state: "unavailable",
			unavailable_reason: "Session policy denies workspace_write",
		});
	});

	it("rejects duplicate and mismatched registration identities during layer construction", async () => {
		const duplicate = ArtisanBuiltInToolRegistrations[0]!;
		const mismatch = { ...duplicate, tool_id: "terminal.read" };
		const duplicate_state_result = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(
				Effect.provide(
					make_artisan_tool_registry_layer().pipe(
						Layer.provide(ArtisanToolApprovalPolicyLive),
						Layer.provide(
							make_artisan_tool_capability_state_layer([
								{ state: "available", tool_id: "terminal.read" },
								{ state: "available", tool_id: "terminal.read" },
							]),
						),
					),
				),
				Effect.flip,
			),
		);
		const duplicate_result = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(
				Effect.provide(make_layer(undefined, [duplicate, duplicate])),
				Effect.flip,
			),
		);
		const mismatch_result = await Effect.runPromise(
			Effect.service(ArtisanToolRegistry).pipe(
				Effect.provide(make_layer(undefined, [mismatch])),
				Effect.flip,
			),
		);

		expect(duplicate_state_result).toMatchObject({ reason: "duplicate_id" });
		expect(duplicate_result).toMatchObject({ reason: "duplicate_id" });
		expect(mismatch_result).toMatchObject({ reason: "mismatched_id" });
	});
});
