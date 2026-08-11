import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import type { EngineOpenInput } from "@artisan/engines";
import { MakeCodexAppServerThreadOptions } from "../../modules/engines/src/codex/internal/permissions";

const full_access_input = {
	_tag: "start",
	artisan_run_id: "full-access-run",
	initial_text: "Create a sibling repository.",
	permission_policy: {
		approval: "never",
		edit_scope: "host",
		network_access: true,
		write_access: true,
	},
	working_directory: "C:\\workspace",
} satisfies EngineOpenInput;

describe("Codex permission mapping", () => {
	it("maps host scope to danger-full-access for app-server", async () => {
		const app_server = await Effect.runPromise(
			MakeCodexAppServerThreadOptions(full_access_input),
		);

		expect(app_server).toEqual({
			approvalPolicy: "never",
			cwd: "C:\\workspace",
			sandbox: "danger-full-access",
		});
	});

	it("passes Ultra through to Codex's advertised app-server effort field", async () => {
		const app_server = await Effect.runPromise(
			MakeCodexAppServerThreadOptions({
				...full_access_input,
				provider_options: { "codex.reasoning_effort": "ultra" },
			}),
		);

		expect(app_server).toMatchObject({
			config: { model_reasoning_effort: "ultra" },
		});
	});

	it("rejects host scope when network isolation is still requested", async () => {
		await expect(
			Effect.runPromise(
				MakeCodexAppServerThreadOptions({
					...full_access_input,
					permission_policy: {
						...full_access_input.permission_policy,
						network_access: false,
					},
				}),
			),
		).rejects.toMatchObject({
			_tag: "EngineConfigurationError",
			option: "permission_policy.network_access",
		});
	});
});
