import { describe, expect, it } from "vitest";

import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog, ThreadSessionPolicy } from "@artisan/protocol";
import {
	ComposerSendBlockedReason,
	type EngineProvisioning,
} from "../../modules/frontend/src/lib/composer/send-readiness";

const engines = [
	{ id: "codex", label: "Codex", model: "codex-sol" },
	{ id: "claude", label: "Claude", model: "claude-fable" },
	{ id: "grok", label: "Grok Build", model: "grok-4-6" },
	{ id: "cursor", label: "Cursor", model: "cursor-composer-2-5" },
	{ id: "hermes", label: "Hermes", model: "hermes-test-route" },
	{ id: "opencode2", label: "OpenCode", model: "opencode2-test" },
] as const;

const policy_for = (engine_id: string, model: string): ThreadSessionPolicy => ({
	engine_id,
	model,
	permission_mode: "on_request",
	reasoning_effort: "high",
	sandbox_mode: "workspace_write",
	strict_clarification: false,
	web_search_enabled: false,
});

const catalog_for = (
	runnable_harness_ids: ReadonlyArray<string>,
	routes?: RuntimeCatalog["routes"],
): RuntimeCatalog => ({
	manifest: model_manifest,
	runnable_harness_ids: [...runnable_harness_ids],
	...(routes === undefined ? {} : { routes }),
});

const managed: EngineProvisioning = {
	active_version: "1.0.0",
	busy: false,
	managed: true,
};

const missing: EngineProvisioning = { busy: false, managed: false };

const missing_after_install: EngineProvisioning = {
	busy: false,
	managed: false,
	previous_version: "0.9.0",
};

describe("composer send readiness matrix", () => {
	it("sends for every runnable engine that is ready", () => {
		for (const engine of engines) {
			const catalog = catalog_for([engine.id]);
			expect(
				ComposerSendBlockedReason(true, catalog, policy_for(engine.id, engine.model)),
				`${engine.id} runnable without provisioning must send`,
			).toBeUndefined();
			expect(
				ComposerSendBlockedReason(
					true,
					catalog,
					policy_for(engine.id, engine.model),
					managed,
				),
				`${engine.id} runnable with a managed binary must send`,
			).toBeUndefined();
		}
	});

	it("stays not-ready for every runnable engine whose binary is missing", () => {
		for (const engine of engines) {
			const catalog = catalog_for([engine.id]);
			expect(
				ComposerSendBlockedReason(
					true,
					catalog,
					policy_for(engine.id, engine.model),
					missing,
				),
			).toBe(`${engine.label} is not set up on this machine yet — install it to send`);
			expect(
				ComposerSendBlockedReason(
					true,
					catalog,
					policy_for(engine.id, engine.model),
					missing_after_install,
				),
			).toBe(`${engine.label}'s installed binary is missing — repair it to send`);
		}
	});

	it("stays preview-only for every unrunnable engine without live signals", () => {
		for (const engine of engines) {
			const catalog = catalog_for([]);
			expect(
				ComposerSendBlockedReason(true, catalog, policy_for(engine.id, engine.model)),
			).toBe(
				`${engine.label} models are preview-only — this engine cannot run in Artisan yet`,
			);
		}
	});

	it("surfaces a failed install instead of reporting ready", () => {
		const catalog = catalog_for(["codex"]);
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("codex", "codex-sol"), {
				busy: false,
				failure: "The managed installation did not complete.",
				managed: false,
			}),
		).toBe("Codex could not start — The managed installation did not complete.");
	});

	it("waits out an in-flight install instead of reporting ready", () => {
		const catalog = catalog_for([]);
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("claude", "claude-fable"), {
				busy: true,
				managed: false,
			}),
		).toBe("Claude is still installing — try again when it finishes");
	});

	it("surfaces unavailable routes with their reason even when runnable", () => {
		const catalog = catalog_for(["hermes"], [
			{
				engine_id: "hermes",
				group: {
					id: "openai-codex",
					label: "OpenAI Codex",
					order: 0,
					show_route_labels: false,
				},
				id: "openai-codex",
				label: "OpenAI Codex",
				status: "unavailable",
				unavailable_reason: "Hermes reports this route is disabled.",
			},
		]);
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("hermes", "hermes-test-route")),
		).toBe("Hermes reports this route is disabled.");
	});

	it("falls back to engine-unavailable when routes carry no reason", () => {
		const catalog = catalog_for(["cursor"], [
			{
				engine_id: "cursor",
				group: { id: "default", label: "Default", order: 0, show_route_labels: false },
				id: "default",
				label: "Default",
				status: "unavailable",
			},
		]);
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("cursor", "cursor-composer-2-5")),
		).toBe("Cursor is unavailable on this Forge right now");
	});

	it("keeps the preview answer when an available route cannot run the harness", () => {
		const catalog = catalog_for([], [
			{
				engine_id: "grok",
				group: { id: "default", label: "Default", order: 0, show_route_labels: false },
				id: "default",
				label: "Default",
				status: "available",
			},
		]);
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("grok", "grok-4-6")),
		).toBe("Grok Build models are preview-only — this engine cannot run in Artisan yet");
	});

	it("surfaces a disabled policy model with the catalog reason", () => {
		const catalog: RuntimeCatalog = {
			manifest: {
				...model_manifest,
				models: model_manifest.models.map((model) =>
					model.id === "claude-fable"
						? {
								...model,
								disabled: { reason: "Claude reports this model is retired." },
							}
						: model,
				),
			},
			runnable_harness_ids: ["claude"],
		};
		expect(
			ComposerSendBlockedReason(true, catalog, policy_for("claude", "claude-fable")),
		).toBe("Claude reports this model is retired.");
	});

	it("keeps Forge-offline and absent-policy precedence", () => {
		const catalog = catalog_for(["codex"]);
		expect(
			ComposerSendBlockedReason(false, catalog, policy_for("codex", "codex-sol"), managed),
		).toBe("Forge is offline — reconnect to send");
		expect(ComposerSendBlockedReason(true, catalog, undefined, missing)).toBeUndefined();
	});
});
