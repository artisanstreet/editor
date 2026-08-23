import type { EnginePermissionPolicy } from "../engine";

export type OpenCode2PermissionLevel = "restricted" | "autonomous" | "unrestricted";

export const OpenCode2AgentId = (
	permission: OpenCode2PermissionLevel,
	network_access: boolean,
	web_search_enabled: boolean,
) =>
	`artisan-v1-${permission}-${network_access ? "network" : "offline"}-${web_search_enabled ? "web" : "no-web"}`;

interface OpenCode2PermissionRule {
	readonly action: string;
	readonly effect: "allow" | "ask" | "deny";
	readonly resource: string;
}

const rule = (
	action: string,
	effect: OpenCode2PermissionRule["effect"],
): OpenCode2PermissionRule => ({ action, effect, resource: "*" });

/** Complete last-match-wins rules; no provider default can widen an omitted action. */
export const OpenCode2PermissionRules = (input: {
	readonly network_access: boolean;
	readonly permission: OpenCode2PermissionLevel;
	readonly web_search_enabled: boolean;
}): ReadonlyArray<OpenCode2PermissionRule> => {
	const rules: Array<OpenCode2PermissionRule> = [rule("*", "deny")];
	for (const action of ["read", "glob", "grep", "question"]) rules.push(rule(action, "allow"));
	if (input.permission !== "restricted") {
		rules.push(rule("edit", "allow"));
	}
	if (input.network_access && input.permission !== "restricted") {
		rules.push(rule("shell", input.permission === "unrestricted" ? "allow" : "ask"));
	}
	if (input.web_search_enabled) {
		rules.push(rule("webfetch", "allow"), rule("websearch", "allow"));
	}
	if (input.permission === "unrestricted") {
		for (const action of ["external_directory", "execute", "mcp", "skill"])
			rules.push(rule(action, "allow"));
	}
	/** Native subagents stay denied even in unrestricted mode for the initial adapter. */
	rules.push(rule("subagent", "deny"));
	return rules;
};

const permission_policy = (level: OpenCode2PermissionLevel): EnginePermissionPolicy => ({
	approval: level === "autonomous" ? "on_request" : "never",
	...(level === "unrestricted" ? { edit_scope: "host" as const } : {}),
	network_access: level === "unrestricted",
	write_access: level !== "restricted",
});

/** Highest-precedence profile overlay supplied through `OPENCODE_CONFIG_CONTENT`. */
export const OpenCode2ManagedConfigContent = () => {
	const agents: Record<string, unknown> = {};
	for (const permission of ["restricted", "autonomous", "unrestricted"] as const) {
		for (const network_access of [false, true]) {
			for (const web_search_enabled of [false, true]) {
				const policy = permission_policy(permission);
				const allowed_network = network_access && policy.write_access;
				const allowed_web = web_search_enabled;
				agents[OpenCode2AgentId(permission, allowed_network, allowed_web)] = {
					description: "Artisan-managed OpenCode execution policy.",
					hidden: true,
					mode: "primary",
					permissions: OpenCode2PermissionRules({
						network_access: allowed_network,
						permission,
						web_search_enabled: allowed_web,
					}),
					system: "Follow Artisan product instructions and session guidance. Never claim that OpenCode permission rules are an operating-system sandbox.",
				};
			}
		}
	}
	return JSON.stringify({
		agents,
		autoupdate: false,
		default_agent: OpenCode2AgentId("autonomous", false, false),
		share: "disabled",
		warming: false,
	});
};
