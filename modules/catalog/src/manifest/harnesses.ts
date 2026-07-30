import type { HarnessDefinition } from "../schema";
import { permission } from "./options";

export const harnesses = [
	{
		id: "codex",
		gateways: [],
		label: "Codex",
		permissions: {
			default: "supervised",
			options: [
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Inspect the workspace without writing files; Codex still applies its read-only sandbox.",
					edit_scope: "none",
					id: "restricted",
					label: "Read only",
					native_value: "read-only",
					safety_boundary: "sandbox",
				}),
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Write inside the workspace while Codex decides when an action needs approval.",
					edit_scope: "workspace",
					id: "supervised",
					label: "Supervised",
					native_value: "workspace-write",
					safety_boundary: "sandbox",
				}),
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Run without approval prompts while remaining inside Codex's workspace-write sandbox.",
					edit_scope: "workspace",
					id: "autonomous",
					label: "Auto approve",
					native_value: "workspace-write-no-prompts",
					safety_boundary: "sandbox",
				}),
			],
		},
	},
	{
		id: "claude",
		gateways: [],
		label: "Claude",
		permissions: {
			default: "supervised",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Explore and propose a plan without editing source files until the plan is approved.",
					edit_scope: "none",
					id: "restricted",
					label: "Plan only",
					native_value: "plan",
					safety_boundary: "plan",
				}),
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Auto-approve reads and ask before actions that require permission.",
					edit_scope: "host",
					id: "supervised",
					label: "Supervised",
					native_value: "default",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Auto-approve in-scope edits and common filesystem operations; prompt for other commands.",
					edit_scope: "host",
					id: "trusted",
					label: "Accept edits",
					native_value: "acceptEdits",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "classifier",
					availability: "dynamic",
					description:
						"Use Anthropic's classifier to run routine actions and block or escalate risky actions.",
					edit_scope: "host",
					id: "autonomous",
					label: "Auto",
					native_value: "auto",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "none",
					availability: "dynamic",
					description:
						"Disable ordinary permission prompts and safety checks; administrator policy may forbid this mode.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Bypass permissions",
					native_value: "bypassPermissions",
					safety_boundary: "bypassed",
				}),
			],
		},
	},
	{
		id: "grok",
		gateways: [],
		label: "Grok Build",
		permissions: {
			default: "supervised",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description: "Prompt for tool calls that are not already allowed by a rule.",
					edit_scope: "host",
					id: "supervised",
					label: "Supervised",
					native_value: "ask",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "classifier",
					availability: "dynamic",
					description:
						"Use xAI's classifier to approve safe tools while dangerous actions may still prompt.",
					edit_scope: "host",
					id: "autonomous",
					label: "Auto",
					native_value: "auto",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Auto-approve tool calls while deny rules and pre-tool hooks remain authoritative.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Always approve",
					native_value: "always-approve",
					safety_boundary: "rules",
				}),
			],
		},
	},
	{
		id: "cursor",
		gateways: [],
		label: "Cursor Agent",
		permissions: {
			default: "supervised",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Use Cursor's interactive default and ask before terminal commands; configured deny rules remain authoritative.",
					edit_scope: "host",
					id: "supervised",
					label: "Supervised",
					native_value: "default",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "none",
					availability: "dynamic",
					description:
						"Run print mode with --force so commands and writes proceed without prompts; explicit deny rules still win.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Force allow",
					native_value: "force",
					safety_boundary: "rules",
				}),
			],
		},
	},
] satisfies ReadonlyArray<HarnessDefinition>;
