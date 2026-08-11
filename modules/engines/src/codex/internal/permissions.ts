import { Effect } from "effect";

import {
	type EngineOpenInput,
	EngineConfigurationError,
	ValidateEngineGlobalGuidance,
	ValidateEngineProductInstructions,
} from "../../engine";

type CodexApprovalPolicy = "never" | "on-request";
type CodexSandbox = "danger-full-access" | "read-only" | "workspace-write";

interface CodexPermissionSettings {
	readonly approval_policy?: CodexApprovalPolicy;
	readonly network_access?: boolean;
	readonly sandbox?: CodexSandbox;
}

const allowed_provider_options = new Set(["codex.reasoning_effort", "codex.service_tier"]);

function FailConfiguration(option: string, value: unknown) {
	return Effect.fail(new EngineConfigurationError({ engine_id: "codex", option, value }));
}

function make_developer_instructions(input: EngineOpenInput) {
	if (input.product_instructions === undefined) return input.global_guidance?.content;
	if (input.global_guidance === undefined) return input.product_instructions.content;

	return [
		"## User global guidance",
		"",
		input.global_guidance.content,
		"",
		"## Artisan product instructions",
		"",
		input.product_instructions.content,
		"",
		"The Artisan product instructions describe the active presentation surface and take precedence over conflicting output-format guidance above.",
	].join("\n");
}

function ResolveCodexPermissions(input: EngineOpenInput) {
	return Effect.gen(function* () {
		const provider_options = input.provider_options ?? {};
		const unknown_option = Object.keys(provider_options).find(
			(option) => !allowed_provider_options.has(option),
		);

		if (unknown_option !== undefined) {
			return yield* FailConfiguration(
				`provider_options.${unknown_option}`,
				provider_options[unknown_option],
			);
		}

		const reasoning_effort = provider_options["codex.reasoning_effort"];
		const service_tier = provider_options["codex.service_tier"];

		if (
			reasoning_effort !== undefined &&
			(typeof reasoning_effort !== "string" ||
				!new Set(["low", "medium", "high", "xhigh", "max", "ultra"]).has(reasoning_effort))
		) {
			return yield* FailConfiguration(
				"provider_options.codex.reasoning_effort",
				reasoning_effort,
			);
		}
		if (
			service_tier !== undefined &&
			(typeof service_tier !== "string" || !new Set(["standard", "fast"]).has(service_tier))
		) {
			return yield* FailConfiguration("provider_options.codex.service_tier", service_tier);
		}

		const policy = input.permission_policy;

		if (policy === undefined) {
			const permissions: CodexPermissionSettings = {};

			return permissions;
		}

		if (!new Set(["never", "on_request", "always"]).has(policy.approval)) {
			return yield* FailConfiguration("permission_policy.approval", policy.approval);
		}

		if (policy.approval === "always") {
			return yield* FailConfiguration("permission_policy.approval", policy.approval);
		}

		if (typeof policy.network_access !== "boolean") {
			return yield* FailConfiguration(
				"permission_policy.network_access",
				policy.network_access,
			);
		}

		if (typeof policy.write_access !== "boolean") {
			return yield* FailConfiguration("permission_policy.write_access", policy.write_access);
		}

		if (
			policy.edit_scope !== undefined &&
			!new Set(["workspace", "host"]).has(policy.edit_scope)
		) {
			return yield* FailConfiguration("permission_policy.edit_scope", policy.edit_scope);
		}

		if (!policy.write_access && policy.edit_scope !== undefined) {
			return yield* FailConfiguration("permission_policy.edit_scope", policy.edit_scope);
		}

		if (policy.network_access && !policy.write_access) {
			return yield* FailConfiguration(
				"permission_policy.network_access",
				policy.network_access,
			);
		}

		const edit_scope = policy.edit_scope ?? "workspace";

		if (edit_scope === "host" && !policy.network_access) {
			return yield* FailConfiguration(
				"permission_policy.network_access",
				policy.network_access,
			);
		}

		const permissions: CodexPermissionSettings = {
			approval_policy: policy.approval === "on_request" ? "on-request" : "never",
			...(policy.write_access && edit_scope === "workspace"
				? { network_access: policy.network_access }
				: {}),
			sandbox: policy.write_access
				? edit_scope === "host"
					? "danger-full-access"
					: "workspace-write"
				: "read-only",
		};

		return permissions;
	});
}

/** Builds app-server thread fields from Artisan's canonical permission policy. */
export function MakeCodexAppServerThreadOptions(input: EngineOpenInput) {
	return ValidateEngineGlobalGuidance("codex", input.global_guidance).pipe(
		Effect.andThen(ValidateEngineProductInstructions("codex", input.product_instructions)),
		Effect.andThen(ResolveCodexPermissions(input)),
		Effect.map((permissions) => {
			const developer_instructions = make_developer_instructions(input);

			return {
				...(permissions.approval_policy === undefined
					? {}
					: { approvalPolicy: permissions.approval_policy }),
				cwd: input.working_directory,
				...(developer_instructions === undefined
					? {}
					: { developerInstructions: developer_instructions }),
				...(input.model === undefined ? {} : { model: input.model }),
				...(input.provider_options?.["codex.service_tier"] === "fast"
					? { serviceTier: "fast" }
					: {}),
				...(permissions.network_access === undefined
					? {}
					: {
							config: {
								sandbox_workspace_write: {
									network_access: permissions.network_access,
								},
							},
						}),
				...(permissions.sandbox === undefined ? {} : { sandbox: permissions.sandbox }),
				...(input.provider_options?.["codex.reasoning_effort"] === undefined
					? {}
					: {
							config: {
								...(permissions.network_access === undefined
									? {}
									: {
											sandbox_workspace_write: {
												network_access: permissions.network_access,
											},
										}),
								model_reasoning_effort:
									input.provider_options["codex.reasoning_effort"],
							},
						}),
			};
		}),
	);
}
