import type { HarnessDefinition } from "../schema";
import { permission } from "./options";

/** Big Pickle's stable base-variant identity in OpenCode's Zen catalog. */
export const opencode2_big_pickle_compaction_model_id =
	"opencode2:eyJtb2RlbF9pZCI6IngtcHJldmlldy1mLWZyZWUiLCJwcm92aWRlcl9yb3V0ZV9pZCI6Im9wZW5jb2RlIn0";

export const harnesses = [
	{
		compaction_default_model_id: "codex-luna",
		id: "codex",
		gateways: [],
		label: "Codex",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Inspect and search without writing files; Codex keeps the read-only sandbox while enabled web search remains available.",
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
						"Work autonomously inside the workspace while Codex's sandbox contains writes and its approval policy handles risky actions.",
					edit_scope: "workspace",
					id: "autonomous",
					label: "Auto",
					native_value: "workspace-write",
					safety_boundary: "sandbox",
				}),
				permission({
					approval_behavior: "none",
					availability: "dynamic",
					description:
						"Remove Codex's local sandbox and approval prompts, granting access beyond the workspace and to the network; administrator policy may forbid this mode.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Unrestricted",
					native_value: "danger-full-access",
					safety_boundary: "bypassed",
				}),
			],
		},
	},
	{
		compaction_default_model_id: "claude-haiku",
		id: "claude",
		gateways: [],
		label: "Claude",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Explore, search, and propose a plan without editing source files until leaving plan mode is approved.",
					edit_scope: "none",
					id: "restricted",
					label: "Read only",
					native_value: "plan",
					safety_boundary: "plan",
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
					label: "Unrestricted",
					native_value: "bypassPermissions",
					safety_boundary: "bypassed",
				}),
			],
		},
	},
	{
		compaction_default_model_id: opencode2_big_pickle_compaction_model_id,
		id: "opencode2",
		gateways: [
			{ id: "opencode", kind: "managed", label: "Zen" },
			{ id: "opencode-go", kind: "managed", label: "Go" },
		],
		label: "OpenCode",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Inspect the selected workspace without mutation, shell, external-directory, or subagent tools; enabled web search remains available.",
					edit_scope: "none",
					id: "restricted",
					label: "Read only",
					native_value: "artisan-restricted",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Allow workspace edits automatically while OpenCode continues to ask before shell access and denies external-directory access.",
					edit_scope: "workspace",
					id: "autonomous",
					label: "Auto",
					native_value: "artisan-auto",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "none",
					availability: "dynamic",
					description:
						"Allow host-level tools without prompts; shell commands have host filesystem, process, and network authority.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Unrestricted",
					native_value: "artisan-unrestricted",
					safety_boundary: "bypassed",
				}),
			],
		},
	},
	{
		id: "hermes",
		gateways: [],
		label: "Hermes",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Use the selected Hermes profile's approval rules. Hermes tools run with host filesystem and network access unless that profile configures an isolated backend.",
					edit_scope: "host",
					id: "autonomous",
					label: "Auto",
					native_value: "profile",
					safety_boundary: "rules",
				}),
				permission({
					approval_behavior: "none",
					availability: "dynamic",
					description:
						"Enable Hermes YOLO for this session. Host-level tools proceed without ordinary approval prompts; Hermes hardline deny rules may still apply.",
					edit_scope: "host",
					id: "unrestricted",
					label: "Unrestricted",
					native_value: "yolo",
					safety_boundary: "bypassed",
				}),
			],
		},
	},
	{
		compaction_default_model_id: "grok-composer-2-5",
		id: "grok",
		gateways: [],
		label: "Grok Build",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Explore, search, and prepare a plan while Grok keeps edit tools limited until leaving plan mode is approved.",
					edit_scope: "none",
					id: "restricted",
					label: "Read only",
					native_value: "plan",
					safety_boundary: "plan",
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
					label: "Unrestricted",
					native_value: "always-approve",
					safety_boundary: "rules",
				}),
			],
		},
	},
	{
		compaction_default_model_id: "cursor-composer-2-5",
		id: "cursor",
		gateways: [],
		label: "Cursor",
		permissions: {
			default: "autonomous",
			options: [
				permission({
					approval_behavior: "none",
					availability: "always",
					description:
						"Use Cursor's read-only Ask mode for questions, exploration, planning, and enabled web search without edits.",
					edit_scope: "none",
					id: "restricted",
					label: "Read only",
					native_value: "ask",
					safety_boundary: "plan",
				}),
				permission({
					approval_behavior: "prompts",
					availability: "always",
					description:
						"Use Cursor's interactive default and ask before terminal commands; configured deny rules remain authoritative.",
					edit_scope: "host",
					id: "autonomous",
					label: "Auto",
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
					label: "Unrestricted",
					native_value: "force",
					safety_boundary: "rules",
				}),
			],
		},
	},
] satisfies ReadonlyArray<HarnessDefinition>;
