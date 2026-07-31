import { Effect } from "effect";

import {
	type EngineOpenInput,
	EngineConfigurationError,
	ValidateEngineGlobalGuidance,
} from "../../engine";

type CodexApprovalPolicy = "never" | "on-request";
type CodexSandbox = "danger-full-access" | "read-only" | "workspace-write";
type CodexTransport = "app-server" | "exec";

interface CodexPermissionSettings {
	readonly approval_policy?: CodexApprovalPolicy;
	readonly exec_profile?: string;
	readonly network_access?: boolean;
	readonly sandbox?: CodexSandbox;
	readonly skip_git_repo_check: boolean;
}

const allowed_provider_options = new Set([
	"codex.exec.profile",
	"codex.exec.skip_git_repo_check",
	"codex.reasoning_effort",
	"codex.service_tier",
]);

function FailConfiguration(option: string, value: unknown) {
	return Effect.fail(new EngineConfigurationError({ engine_id: "codex", option, value }));
}

function toml_string(value: string) {
	return JSON.stringify(value);
}

function ResolveCodexPermissions(input: EngineOpenInput, transport: CodexTransport) {
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

		const exec_profile = provider_options["codex.exec.profile"];
		const skip_git_repo_check = provider_options["codex.exec.skip_git_repo_check"];
		const reasoning_effort = provider_options["codex.reasoning_effort"];
		const service_tier = provider_options["codex.service_tier"];

		if (
			exec_profile !== undefined &&
			(typeof exec_profile !== "string" || exec_profile.length === 0)
		) {
			return yield* FailConfiguration("provider_options.codex.exec.profile", exec_profile);
		}

		if (skip_git_repo_check !== undefined && typeof skip_git_repo_check !== "boolean") {
			return yield* FailConfiguration(
				"provider_options.codex.exec.skip_git_repo_check",
				skip_git_repo_check,
			);
		}
		if (
			reasoning_effort !== undefined &&
			(typeof reasoning_effort !== "string" ||
				!new Set(["low", "medium", "high", "xhigh", "max"]).has(reasoning_effort))
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
		if (
			transport === "app-server" &&
			(exec_profile !== undefined || skip_git_repo_check !== undefined)
		) {
			const option =
				exec_profile !== undefined
					? "codex.exec.profile"
					: "codex.exec.skip_git_repo_check";

			return yield* FailConfiguration(`provider_options.${option}`, provider_options[option]);
		}

		const policy = input.permission_policy;

		if (policy === undefined) {
			const permissions: CodexPermissionSettings = {
				...(exec_profile === undefined ? {} : { exec_profile }),
				skip_git_repo_check: skip_git_repo_check === true,
			};

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
			...(exec_profile === undefined ? {} : { exec_profile }),
			...(policy.write_access && edit_scope === "workspace"
				? { network_access: policy.network_access }
				: {}),
			sandbox: policy.write_access
				? edit_scope === "host"
					? "danger-full-access"
					: "workspace-write"
				: "read-only",
			skip_git_repo_check: skip_git_repo_check === true,
		};

		return permissions;
	});
}

/** Builds app-server thread fields from Artisan's canonical permission policy. */
export function MakeCodexAppServerThreadOptions(input: EngineOpenInput) {
	return ValidateEngineGlobalGuidance("codex", input.global_guidance).pipe(
		Effect.andThen(ResolveCodexPermissions(input, "app-server")),
		Effect.map((permissions) => ({
			...(permissions.approval_policy === undefined
				? {}
				: { approvalPolicy: permissions.approval_policy }),
			cwd: input.working_directory,
			...(input.global_guidance === undefined
				? {}
				: { developerInstructions: input.global_guidance.content }),
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
		})),
	);
}

/** Builds Codex exec permission and provider-option argv without a shell. */
export function MakeCodexExecPermissionArgs(input: EngineOpenInput) {
	return ResolveCodexPermissions(input, "exec").pipe(
		Effect.map((permissions) => [
			...(permissions.approval_policy === undefined
				? []
				: ["-c", `approval_policy=${toml_string(permissions.approval_policy)}`]),
			...(permissions.network_access === undefined
				? []
				: [
						"-c",
						`sandbox_workspace_write.network_access=${String(permissions.network_access)}`,
					]),
			...(input.provider_options?.["codex.reasoning_effort"] === undefined
				? []
				: [
						"-c",
						`model_reasoning_effort=${toml_string(input.provider_options["codex.reasoning_effort"] as string)}`,
					]),
			...(permissions.exec_profile === undefined
				? []
				: ["--profile", permissions.exec_profile]),
			...(permissions.sandbox === undefined ? [] : ["--sandbox", permissions.sandbox]),
			...(permissions.skip_git_repo_check ? ["--skip-git-repo-check"] : []),
		]),
	);
}
