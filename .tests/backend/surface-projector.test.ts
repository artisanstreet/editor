import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	surface_raw_origin_identifier_maximum_bytes,
	type EventPayload,
	type SurfaceItem,
} from "@artisan/protocol";

import {
	SurfaceProjector,
	SurfaceProjectorLive,
} from "../../modules/backend/src/surface/surface-projector";

const timestamp = "2026-07-16T12:00:00.000Z";
const head = "a".repeat(40);
const hash = "b".repeat(64);
const thread_id = "thread_1";
const envelope_agent_id = "agent_envelope";
const envelope_run_id = "run_envelope";
const envelope_raw_origin = { provider: "engine_1", reference: "opaque_1" } as const;

interface ExpectedSurfaceInput {
	readonly agent_id?: string;
	readonly group: SurfaceItem["group"];
	readonly kind: SurfaceItem["kind"];
	readonly label: string;
	readonly project_id?: string;
	readonly run_id?: string;
	readonly source?: SurfaceItem["source"];
	readonly state: string;
	readonly summary: string;
	readonly surface_id: string;
	readonly usage?: SurfaceItem["usage"];
	readonly workspace_id?: string;
}

interface EnvelopeOptions {
	readonly agent_id?: string;
	readonly include_raw_origin?: boolean;
	readonly journal_sequence?: number;
	readonly message_id?: string;
	readonly run_id?: false | string;
	readonly sequence?: number;
	readonly stream_id?: string;
}

type EventFixtureMap = {
	readonly [Type in EventPayload["type"]]: {
		readonly expected: SurfaceItem | undefined;
		readonly payload: Extract<EventPayload, { readonly type: Type }>;
	};
};

function expected_surface(input: ExpectedSurfaceInput): SurfaceItem {
	return {
		agent_id: input.agent_id ?? envelope_agent_id,
		group: input.group,
		kind: input.kind,
		label: input.label,
		...(input.project_id === undefined ? {} : { project_id: input.project_id }),
		raw_origin: envelope_raw_origin,
		run_id: input.run_id ?? envelope_run_id,
		source: input.source ?? "artisan",
		state: input.state,
		summary: input.summary,
		surface_id: input.surface_id,
		thread_id,
		timestamp,
		...(input.usage === undefined ? {} : { usage: input.usage }),
		...(input.workspace_id === undefined ? {} : { workspace_id: input.workspace_id }),
	};
}

function event_envelope(payload: EventPayload, options: EnvelopeOptions = {}) {
	const journal_sequence = options.journal_sequence ?? 1;

	return {
		agent_id: options.agent_id ?? envelope_agent_id,
		causation_id: "cause_1",
		correlation_id: "correlation_1",
		journal_sequence,
		kind: "event" as const,
		message_id: options.message_id ?? `message_${journal_sequence}`,
		origin: "backend" as const,
		payload,
		protocol_version: 1 as const,
		...(options.include_raw_origin === false ? {} : { raw_origin: envelope_raw_origin }),
		...(options.run_id === false ? {} : { run_id: options.run_id ?? envelope_run_id }),
		schema_version: 1 as const,
		sent_at: timestamp,
		sequence: options.sequence ?? journal_sequence,
		stream_id: options.stream_id ?? "stream_1",
		thread_id,
	};
}

const project_ref = {
	display_name: "Artisan Editor",
	project_id: "project_thread",
	root_path: "C:/Projects/artisan-editor",
} as const;

const thread = {
	activity_version: 1,
	affinity_version: 1,
	created_at: timestamp,
	last_activity_at: timestamp,
	linked_projects: [],
	live_status: "Working",
	metadata_version: 1,
	pinned: false,
	primary_project: project_ref,
	project_affinity_scores: [],
	project_locked: false,
	thread_id,
	title: "Surface taxonomy",
	title_locked: false,
	title_source: "initial" as const,
	updated_at: timestamp,
};

const content_identity = {
	algorithm: "sha256" as const,
	byte_count: 4,
	content_hash: hash,
};

const workspace_change = {
	after_identity: content_identity,
	agent_id: "agent_change",
	before_identity: content_identity,
	change_id: "change_1",
	created_at: timestamp,
	path: "src/main.ts",
	raw_origin: { provider: "payload_engine", reference: "payload_private_origin" },
	review_state: "needs_review" as const,
	rollback_state: "available" as const,
	run_id: "run_change",
	source_command_id: "command_change",
	thread_id,
	updated_at: timestamp,
	version: 1,
	workspace_id: "workspace_change",
};

const workspace_replace_approval = {
	after_identity: content_identity,
	agent_id: "agent_replace",
	approval_id: "approval_replace",
	before_identity: content_identity,
	change_id: "change_replace",
	created_at: timestamp,
	path: "src/main.ts",
	policy: "on_request" as const,
	reason: "Apply the reviewed workspace replacement.",
	run_id: "run_replace",
	state: "requested" as const,
	thread_id,
	updated_at: timestamp,
	workspace_id: "workspace_replace",
};

const git_session = {
	blockers: [],
	branch: "main",
	changed_files: [],
	diff_stats: { additions: 0, deletions: 0, files: 0 },
	has_diff: false,
	head,
	journal_sequence: 1,
	observed_at: timestamp,
	state: "ready" as const,
	version: 1,
	worktrees: [
		{
			bare: false,
			branch: "main",
			detached: false,
			head,
			locked: false,
			location: "selected" as const,
			prunable: false,
		},
	],
	workspace_id: "workspace_git",
};

const git_checkout_approval = {
	approval_id: "approval_checkout",
	created_at: timestamp,
	expected_session_version: 1,
	source_branch: "main",
	source_command_id: "command_checkout",
	source_head: head,
	state: "requested" as const,
	target_branch: "feature/surface",
	thread_id,
	updated_at: timestamp,
	workspace_id: "workspace_git",
};

const git_mutation_approval = {
	approval_id: "approval_git_mutation",
	created_at: timestamp,
	expected_session_version: 1,
	operation: { type: "commit" as const },
	source_branch: "main",
	source_command_id: "command_git_mutation",
	source_head: head,
	state: "requested" as const,
	thread_id,
	updated_at: timestamp,
	workspace_id: "workspace_git",
};

const hosted_repository = {
	host: "github.com",
	name: "artisan-editor",
	owner: "artisan",
	provider_id: "github",
} as const;

const pull_request_origin = {
	native_id: "PR_1",
	provider_id: "github",
	resource_kind: "pull_request" as const,
};

const hosted_mutation_operation = {
	expected_head_commit: head,
	method: "squash" as const,
	operation: "merge_pull_request" as const,
	pull_request_number: 1,
	pull_request_origin,
	repository: hosted_repository,
	selected_branch: "feature/surface",
	snapshot_version: 1,
	workspace_id: "workspace_git",
};

const hosted_mutation_approval = {
	approval_id: "approval_hosted_mutation",
	created_at: timestamp,
	expected_head_commit: head,
	operation: hosted_mutation_operation,
	pull_request_number: 1,
	pull_request_origin,
	repository: hosted_repository,
	selection: { account_login: "artisan", host: "github.com", provider_id: "github" },
	snapshot_version: 1,
	source_command_id: "command_hosted_mutation",
	state: "requested" as const,
	thread_id,
	updated_at: timestamp,
	workspace_id: "workspace_git",
};

const hosted_snapshot = {
	journal_sequence: 1,
	lookup: {
		association: { _tag: "none" as const },
		branch: "main",
		expected_head_commit: head,
		repository: hosted_repository,
	},
	observed_at: timestamp,
	project_id: "project_hosted",
	version: 1,
	workspace_freshness: "current" as const,
	workspace_id: "workspace_git",
};

const hosted_clone_approval = {
	approval_id: "approval_clone",
	created_at: timestamp,
	destination_path: "C:\\Projects\\artisan-editor",
	repository: {
		host: "github.com",
		name: "artisan-editor",
		owner: "artisan",
		provider_id: "github",
		selected_account_login: "artisan",
		web_url: "https://github.com/artisan/artisan-editor",
	},
	source_command_id: "command_clone",
	state: "requested" as const,
	thread_id,
	updated_at: timestamp,
};

const external_wait_snapshot = {
	baseline_fingerprint: hash,
	created_at: timestamp,
	gates: [{ _tag: "required_checks_terminal" as const }],
	generation: 1,
	journal_sequence: 1,
	maximum_generation: 3,
	owner: {
		_tag: "thread_run" as const,
		agent_id: "agent_wait",
		engine_id: "engine_1",
		run_id: "run_wait",
	},
	project_id: "project_wait",
	state: { _tag: "waiting" as const },
	target: {
		branch: "feature/surface",
		expected_head_commit: head,
		pull_request_number: 1,
		pull_request_origin,
		repository: hosted_repository,
	},
	thread_id,
	updated_at: timestamp,
	version: 1,
	wait_id: "wait_1",
	workspace_id: "workspace_wait",
};

const terminal = {
	args: [],
	cols: 120,
	created_at: timestamp,
	executable: "pwsh.exe",
	generation: 1,
	rows: 30,
	state: "active" as const,
	terminal_id: "terminal_1",
	thread_id,
	updated_at: timestamp,
	workspace_id: "workspace_terminal",
	working_directory: "C:/Projects/artisan-editor",
};

const artifact = {
	artifact_id: "artifact_1",
	assignment_id: "assignment_1",
	content: "PRIVATE_ARTIFACT_CONTENT",
	created_at: timestamp,
	group_id: "group_1",
	kind: "summary" as const,
	label: "Private provider summary label",
	raw_origin: { provider: "payload_engine", reference: "payload_artifact_origin" },
	run_id: "run_artifact",
};

const preview_target = {
	created_at_ms: 1_000,
	project_id: "project_preview",
	state: "registered" as const,
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "http://127.0.0.1:4173/",
	workspace_id: "workspace_preview",
};

const preview_launch = {
	initiator: { kind: "user" as const },
	launch_id: "launch_1",
	project_id: "project_preview",
	requested_at_ms: 1_000,
	state: "dispatched" as const,
	target_generation_id: "generation_1",
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "http://127.0.0.1:4173/",
	workspace_id: "workspace_preview",
};

const preview_inspection = {
	connector_id: "connector_1",
	initiator: { agent_id: "agent_preview", kind: "agent" as const },
	inspection_id: "inspection_1",
	project_id: "project_preview",
	requested_at_ms: 1_000,
	state: "attached" as const,
	target_generation_id: "generation_1",
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "https://localhost:4173/",
	workspace_id: "workspace_preview",
};

const tool_context = {
	agent_id: "agent_tool",
	run_id: "run_tool",
	thread_id,
	workspace_id: "workspace_tool",
} as const;

const invocation_tool = {
	approval_policy: "automatic" as const,
	effect: "read" as const,
	label: "Read workspace metadata",
	revision: 1,
	source: "marketplace" as const,
	summary: "Reads curated workspace metadata.",
	tool_id: "marketplace.workspace.read",
};

const tool_invocation = {
	context: tool_context,
	created_at: timestamp,
	invocation_id: "invocation_shared",
	request_id: "request_tool",
	settled_at: timestamp,
	started_at: timestamp,
	state: "completed" as const,
	tool: invocation_tool,
	updated_at: timestamp,
};

const approval_tool = {
	approval_policy: "required" as const,
	effect: "workspace_mutation" as const,
	label: "Apply workspace change",
	revision: 1,
	source: "marketplace" as const,
	summary: "Applies an approved workspace change.",
	tool_id: "marketplace.workspace.apply",
};

const tool_approval = {
	approval_id: "approval_tool",
	context: tool_context,
	created_at: timestamp,
	invocation_id: "invocation_approval",
	request_id: "request_approval",
	state: "requested" as const,
	tool: approval_tool,
	updated_at: timestamp,
};

const event_fixtures = {
	"agent_instance.renamed": {
		expected: expected_surface({
			agent_id: "agent_renamed",
			group: "Agents",
			kind: "agent",
			label: "Agent",
			state: "renamed",
			summary: "Agent renamed.",
			surface_id: "surface:agent:agent_renamed",
		}),
		payload: {
			agent_id: "agent_renamed",
			display_name: "Terra",
			group_id: "group_1",
			type: "agent_instance.renamed",
		},
	},
	"artifact.recorded": {
		expected: expected_surface({
			group: "Knowledge",
			kind: "knowledge",
			label: "Knowledge",
			run_id: "run_artifact",
			state: "recorded",
			summary: "Knowledge captured.",
			surface_id: "surface:knowledge:artifact_1",
		}),
		payload: { artifact, group_id: "group_1", type: "artifact.recorded" },
	},
	"assignment.control": {
		expected: expected_surface({
			group: "Agents",
			kind: "agent",
			label: "Agent",
			state: "accepted",
			summary: "Agent updated.",
			surface_id: "surface:assignment:assignment_1",
		}),
		payload: {
			action: "steer",
			assignment_id: "assignment_1",
			group_id: "group_1",
			outcome: "accepted",
			type: "assignment.control",
		},
	},
	"assignment.heartbeat": {
		expected: expected_surface({
			group: "Agents",
			kind: "agent",
			label: "Agent",
			state: "updated",
			summary: "Agent updated.",
			surface_id: "surface:assignment:assignment_1",
		}),
		payload: {
			assignment_id: "assignment_1",
			group_id: "group_1",
			heartbeat: {
				confidence: 0.8,
				current_action: "Running focused tests",
				short_description: "Surface projection",
				updated_at: timestamp,
			},
			type: "assignment.heartbeat",
		},
	},
	"assistant.message_completed": {
		expected: expected_surface({
			group: "Work",
			kind: "message",
			label: "Message",
			state: "completed",
			summary: "Message completed.",
			surface_id: "surface:message:message_assistant",
		}),
		payload: {
			message_id: "message_assistant",
			text: "Completed the surface projection.",
			type: "assistant.message_completed",
		},
	},
	"capability.invocation.updated": {
		expected: expected_surface({
			group: "Capabilities",
			kind: "capability",
			label: "Search workspace",
			source: "engine",
			state: "completed",
			summary: "Searches the selected workspace.",
			surface_id: "surface:capability:invocation_shared",
		}),
		payload: {
			effect: "read",
			invocation_id: "invocation_shared",
			label: "Search workspace",
			source: "engine",
			state: "completed",
			summary: "Searches the selected workspace.",
			type: "capability.invocation.updated",
		},
	},
	"engine.native_action.observed": {
		expected: expected_surface({
			group: "Capabilities",
			kind: "capability",
			label: "Search workspace",
			source: "engine",
			state: "observed",
			summary: "Observed a native search without private output.",
			surface_id: "surface:native-action:native_1",
		}),
		payload: {
			action_id: "native_1",
			effect: "unknown",
			label: "Search workspace",
			source: "engine",
			state: "observed",
			summary: "Observed a native search without private output.",
			type: "engine.native_action.observed",
		},
	},
	"external_wait.updated": {
		expected: expected_surface({
			agent_id: "agent_wait",
			group: "Time",
			kind: "timer",
			label: "Timer",
			project_id: "project_wait",
			run_id: "run_wait",
			state: "waiting",
			summary: "Timer updated.",
			surface_id: "surface:timer:wait_1",
			workspace_id: "workspace_wait",
		}),
		payload: { snapshot: external_wait_snapshot, type: "external_wait.updated" },
	},
	"filesystem.mutation": {
		expected: expected_surface({
			group: "Changes",
			kind: "change",
			label: "Change",
			state: "write",
			summary: "Change updated.",
			surface_id: "surface:change:filesystem:cause_1",
		}),
		payload: {
			operation: "write",
			path: "C:/Projects/artisan-editor/src/main.ts",
			type: "filesystem.mutation",
		},
	},
	"git.workspace.observed": {
		expected: expected_surface({
			group: "Workspace",
			kind: "workspace",
			label: "Workspace",
			state: "changed",
			summary: "Workspace observed.",
			surface_id: "surface:workspace:observation:cause_1",
		}),
		payload: {
			branch: "feature/surface",
			changed_file_count: 2,
			has_diff: true,
			root_path: "C:/Projects/artisan-editor",
			type: "git.workspace.observed",
			worktree_path: "C:/Projects/artisan-editor",
		},
	},
	"guidance.canonical.updated": {
		expected: expected_surface({
			group: "Guidance",
			kind: "guidance",
			label: "Guidance",
			state: "updated",
			summary: "Guidance updated.",
			surface_id: "surface:guidance:global",
		}),
		payload: {
			byte_count: 42,
			content_hash: hash,
			type: "guidance.canonical.updated",
		},
	},
	"guidance.provider.reconciled": {
		expected: expected_surface({
			group: "Guidance",
			kind: "guidance",
			label: "Guidance",
			state: "synced",
			summary: "Guidance reconciled.",
			surface_id: "surface:guidance:global",
		}),
		payload: {
			provider: "codex",
			status: "synced",
			type: "guidance.provider.reconciled",
		},
	},
	"guidance.selection.required": {
		expected: expected_surface({
			group: "Guidance",
			kind: "guidance",
			label: "Guidance",
			state: "selection_required",
			summary: "Guidance selection required.",
			surface_id: "surface:guidance:global",
		}),
		payload: { candidate_hashes: [hash], type: "guidance.selection.required" },
	},
	"hosted.git.mutation.approval.updated": {
		expected: expected_surface({
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_hosted_mutation",
			workspace_id: "workspace_git",
		}),
		payload: {
			approval: hosted_mutation_approval,
			type: "hosted.git.mutation.approval.updated",
		},
	},
	"hosted.git.snapshot.updated": {
		expected: expected_surface({
			group: "Workspace",
			kind: "workspace",
			label: "Workspace",
			project_id: "project_hosted",
			state: "current",
			summary: "Workspace updated.",
			surface_id: "surface:workspace:workspace_git",
			workspace_id: "workspace_git",
		}),
		payload: { snapshot: hosted_snapshot, type: "hosted.git.snapshot.updated" },
	},
	"hosted.project.clone.approval.updated": {
		expected: expected_surface({
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_clone",
		}),
		payload: {
			approval: hosted_clone_approval,
			type: "hosted.project.clone.approval.updated",
		},
	},
	"interaction.approval": {
		expected: expected_surface({
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_interaction",
		}),
		payload: {
			approval_id: "approval_interaction",
			description: "Approve the canonical action.",
			state: "requested",
			type: "interaction.approval",
		},
	},
	"interaction.question": {
		expected: expected_surface({
			group: "Work",
			kind: "question",
			label: "Question",
			state: "requested",
			summary: "Question updated.",
			surface_id: "surface:question:question_1",
		}),
		payload: {
			question_id: "question_1",
			state: "requested",
			text: "Which project should own this thread?",
			type: "interaction.question",
		},
	},
	"model_behaviour.provider.reconciled": {
		expected: expected_surface({
			group: "Settings",
			kind: "setting",
			label: "Setting",
			state: "synced",
			summary: "Setting reconciled.",
			surface_id: "surface:setting:model-behaviour:auto_compaction_trigger_tokens",
		}),
		payload: {
			provider_id: "engine_1",
			setting_id: "auto_compaction_trigger_tokens",
			status: "synced",
			type: "model_behaviour.provider.reconciled",
		},
	},
	"model_behaviour.setting.updated": {
		expected: expected_surface({
			group: "Settings",
			kind: "setting",
			label: "Setting",
			state: "updated",
			summary: "Setting updated.",
			surface_id: "surface:setting:model-behaviour:auto_compaction_trigger_tokens",
		}),
		payload: {
			setting_id: "auto_compaction_trigger_tokens",
			type: "model_behaviour.setting.updated",
			value: { type: "provider_default" },
			version: 1,
		},
	},
	"orchestration.graph.lifecycle": {
		expected: expected_surface({
			group: "Agents",
			kind: "agent",
			label: "Agent",
			state: "running",
			summary: "Agent updated.",
			surface_id: "surface:assignment:assignment_1",
		}),
		payload: {
			action: "started",
			attempt: 1,
			group_id: "group_1",
			node_id: "assignment_1",
			node_type: "assignment",
			state: "running",
			type: "orchestration.graph.lifecycle",
		},
	},
	"preview.browser.launch.updated": {
		expected: expected_surface({
			group: "Workspace",
			kind: "preview",
			label: "Preview",
			project_id: "project_preview",
			state: "dispatched",
			summary: "Preview updated.",
			surface_id: "surface:preview-launch:launch_1",
			workspace_id: "workspace_preview",
		}),
		payload: {
			action: "dispatched",
			launch: preview_launch,
			type: "preview.browser.launch.updated",
		},
	},
	"preview.inspection.updated": {
		expected: expected_surface({
			agent_id: "agent_preview",
			group: "Workspace",
			kind: "preview",
			label: "Preview",
			project_id: "project_preview",
			state: "attached",
			summary: "Preview updated.",
			surface_id: "surface:preview-inspection:inspection_1",
			workspace_id: "workspace_preview",
		}),
		payload: {
			action: "attached",
			inspection: preview_inspection,
			type: "preview.inspection.updated",
		},
	},
	"preview.target.updated": {
		expected: expected_surface({
			group: "Workspace",
			kind: "preview",
			label: "Preview",
			project_id: "project_preview",
			state: "registered",
			summary: "Preview updated.",
			surface_id: "surface:preview:target_1",
			workspace_id: "workspace_preview",
		}),
		payload: {
			action: "registered",
			target: preview_target,
			type: "preview.target.updated",
		},
	},
	"process.ownership": {
		expected: expected_surface({
			group: "Processes",
			kind: "process",
			label: "Process",
			source: "engine",
			state: "observed",
			summary: "Process observed.",
			surface_id: "surface:process:ownership:cause_1",
		}),
		payload: {
			source: "engine",
			type: "process.ownership",
			working_directory: "C:/Projects/artisan-editor",
		},
	},
	"run.lifecycle": {
		expected: expected_surface({
			group: "Work",
			kind: "run",
			label: "Run",
			state: "running",
			summary: "Run updated.",
			surface_id: "surface:run:run_envelope",
		}),
		payload: {
			state: "running",
			type: "run.lifecycle",
			working_directory: "C:/Projects/artisan-editor",
		},
	},
	"run.usage.updated": {
		expected: expected_surface({
			group: "Work",
			kind: "run",
			label: "Run",
			state: "updated",
			summary: "Run usage updated.",
			surface_id: "surface:run:run_envelope",
			usage: { input_tokens: 1_024, output_tokens: 256 },
		}),
		payload: {
			type: "run.usage.updated",
			usage: { input_tokens: 1_024, output_tokens: 256 },
		},
	},
	"terminal.lifecycle": {
		expected: expected_surface({
			group: "Processes",
			kind: "process",
			label: "Process",
			state: "opened",
			summary: "Process updated.",
			surface_id: "surface:process:terminal:terminal_1",
			workspace_id: "workspace_terminal",
		}),
		payload: { action: "opened", terminal, type: "terminal.lifecycle" },
	},
	"thread.content_erased": {
		expected: undefined,
		payload: { type: "thread.content_erased" },
	},
	"thread.created": {
		expected: expected_surface({
			group: "Work",
			kind: "thread",
			label: "Thread",
			state: "created",
			summary: "Thread created.",
			surface_id: "surface:thread:thread_1",
		}),
		payload: { title: "Surface taxonomy", type: "thread.created" },
	},
	"thread.erased": {
		expected: undefined,
		payload: { type: "thread.erased" },
	},
	"thread.message_queued": {
		expected: expected_surface({
			group: "Work",
			kind: "message",
			label: "Message",
			state: "queued",
			summary: "Message queued.",
			surface_id: "surface:message:message_user",
		}),
		payload: {
			message_id: "message_user",
			reason: "no_active_run",
			text: "Implement the surface layer.",
			type: "thread.message_queued",
			working_directory: "C:/Projects/artisan-editor",
		},
	},
	"thread.message_steering": {
		expected: expected_surface({
			group: "Work",
			kind: "message",
			label: "Message",
			state: "steering",
			summary: "Message sent.",
			surface_id: "surface:message:message_steering",
		}),
		payload: {
			message_id: "message_steering",
			text: "Include run usage.",
			type: "thread.message_steering",
			working_directory: "C:/Projects/artisan-editor",
		},
	},
	"thread.metadata.updated": {
		expected: expected_surface({
			group: "Work",
			kind: "thread",
			label: "Thread",
			project_id: "project_thread",
			state: "metadata",
			summary: "Thread updated.",
			surface_id: "surface:thread:thread_1",
		}),
		payload: { change: "metadata", thread, type: "thread.metadata.updated" },
	},
	"thread.project_affinity.ignored": {
		expected: undefined,
		payload: {
			basis_affinity_version: 1,
			reason: "locked",
			type: "thread.project_affinity.ignored",
		},
	},
	"thread.project_affinity.updated": {
		expected: expected_surface({
			group: "Work",
			kind: "thread",
			label: "Thread",
			project_id: "project_thread",
			state: "observed",
			summary: "Thread updated.",
			surface_id: "surface:thread:thread_1",
		}),
		payload: {
			change: "observed",
			thread,
			type: "thread.project_affinity.updated",
		},
	},
	"thread.refinement.ignored": {
		expected: undefined,
		payload: {
			basis_activity_version: 1,
			basis_metadata_version: 1,
			type: "thread.refinement.ignored",
		},
	},
	"thread.retention.updated": {
		expected: expected_surface({
			group: "Settings",
			kind: "setting",
			label: "Setting",
			state: "enabled",
			summary: "Setting updated.",
			surface_id: "surface:setting:thread-retention",
		}),
		payload: {
			policy: { enabled: true, inactivity_days: 30 },
			type: "thread.retention.updated",
		},
	},
	"tool.approval.updated": {
		expected: expected_surface({
			agent_id: "agent_tool",
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			run_id: "run_tool",
			source: "marketplace",
			state: "requested",
			summary: "Applies an approved workspace change.",
			surface_id: "surface:approval:approval_tool",
			workspace_id: "workspace_tool",
		}),
		payload: { approval: tool_approval, type: "tool.approval.updated" },
	},
	"tool.invocation.updated": {
		expected: expected_surface({
			agent_id: "agent_tool",
			group: "Capabilities",
			kind: "capability",
			label: "Read workspace metadata",
			run_id: "run_tool",
			source: "marketplace",
			state: "completed",
			summary: "Reads curated workspace metadata.",
			surface_id: "surface:capability:invocation_shared",
			workspace_id: "workspace_tool",
		}),
		payload: { invocation: tool_invocation, type: "tool.invocation.updated" },
	},
	"workspace.change.updated": {
		expected: expected_surface({
			agent_id: "agent_change",
			group: "Changes",
			kind: "change",
			label: "Change",
			run_id: "run_change",
			state: "needs_review",
			summary: "Change updated.",
			surface_id: "surface:change:change_1",
			workspace_id: "workspace_change",
		}),
		payload: { action: "recorded", change: workspace_change, type: "workspace.change.updated" },
	},
	"workspace.git.checkout.approval.updated": {
		expected: expected_surface({
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_checkout",
			workspace_id: "workspace_git",
		}),
		payload: {
			approval: git_checkout_approval,
			type: "workspace.git.checkout.approval.updated",
		},
	},
	"workspace.git.fetch.completed": {
		expected: expected_surface({
			group: "Workspace",
			kind: "workspace",
			label: "Workspace",
			state: "failed",
			summary: "Workspace updated.",
			surface_id: "surface:workspace:workspace_git",
			workspace_id: "workspace_git",
		}),
		payload: {
			attempt: { attempted_at: timestamp, result: "failed" },
			type: "workspace.git.fetch.completed",
			workspace_id: "workspace_git",
		},
	},
	"workspace.git.fetch.policy.updated": {
		expected: expected_surface({
			group: "Settings",
			kind: "setting",
			label: "Setting",
			state: "enabled",
			summary: "Setting updated.",
			surface_id: "surface:setting:workspace-git-fetch",
		}),
		payload: { enabled: true, type: "workspace.git.fetch.policy.updated" },
	},
	"workspace.git.fetch.requested": {
		expected: expected_surface({
			group: "Workspace",
			kind: "workspace",
			label: "Workspace",
			state: "fetch_requested",
			summary: "Workspace updated.",
			surface_id: "surface:workspace:workspace_git",
			workspace_id: "workspace_git",
		}),
		payload: { type: "workspace.git.fetch.requested", workspace_id: "workspace_git" },
	},
	"workspace.git.mutation.approval.updated": {
		expected: expected_surface({
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_git_mutation",
			workspace_id: "workspace_git",
		}),
		payload: {
			approval: git_mutation_approval,
			type: "workspace.git.mutation.approval.updated",
		},
	},
	"workspace.git.session.updated": {
		expected: expected_surface({
			group: "Workspace",
			kind: "workspace",
			label: "Workspace",
			state: "ready",
			summary: "Workspace updated.",
			surface_id: "surface:workspace:workspace_git",
			workspace_id: "workspace_git",
		}),
		payload: { session: git_session, type: "workspace.git.session.updated" },
	},
	"workspace.replace.approval.updated": {
		expected: expected_surface({
			agent_id: "agent_replace",
			group: "Permissions",
			kind: "approval",
			label: "Approval",
			run_id: "run_replace",
			state: "requested",
			summary: "Approval updated.",
			surface_id: "surface:approval:approval_replace",
			workspace_id: "workspace_replace",
		}),
		payload: {
			approval: workspace_replace_approval,
			type: "workspace.replace.approval.updated",
		},
	},
} satisfies EventFixtureMap;

const fixture_entries = Object.entries(event_fixtures).map(([type, fixture], index) => ({
	fixture,
	index,
	type,
}));

const Project = (input: unknown) =>
	Effect.gen(function* () {
		const projector = yield* SurfaceProjector;

		return yield* projector.Project(input);
	}).pipe(Effect.provide(SurfaceProjectorLive));

const ProjectMany = (inputs: ReadonlyArray<unknown>) =>
	Effect.gen(function* () {
		const projector = yield* SurfaceProjector;

		return yield* projector.ProjectMany(inputs);
	}).pipe(Effect.provide(SurfaceProjectorLive));

describe("surface projector", () => {
	it.each(fixture_entries)("strictly classifies $type", async ({ fixture, index, type }) => {
		expect(fixture.payload.type).toBe(type);

		const projected = await Effect.runPromise(
			Project(event_envelope(fixture.payload, { journal_sequence: index + 1 })),
		);

		expect(projected).toEqual(fixture.expected === undefined ? [] : [fixture.expected]);
	});

	it("shares stable IDs across paired capability and run lifecycle events", async () => {
		const [capability, tool, lifecycle, usage] = await Promise.all([
			Effect.runPromise(
				Project(
					event_envelope(event_fixtures["capability.invocation.updated"].payload, {
						journal_sequence: 50,
						message_id: "paired_capability",
					}),
				),
			),
			Effect.runPromise(
				Project(
					event_envelope(event_fixtures["tool.invocation.updated"].payload, {
						journal_sequence: 51,
						message_id: "paired_tool",
					}),
				),
			),
			Effect.runPromise(
				Project(
					event_envelope(event_fixtures["run.lifecycle"].payload, {
						journal_sequence: 52,
						message_id: "paired_run_lifecycle",
					}),
				),
			),
			Effect.runPromise(
				Project(
					event_envelope(event_fixtures["run.usage.updated"].payload, {
						journal_sequence: 53,
						message_id: "paired_run_usage",
					}),
				),
			),
		] as const);

		expect(capability[0]?.surface_id).toBe("surface:capability:invocation_shared");
		expect(tool[0]?.surface_id).toBe(capability[0]?.surface_id);
		expect(lifecycle[0]?.surface_id).toBe("surface:run:run_envelope");
		expect(usage[0]?.surface_id).toBe(lifecycle[0]?.surface_id);
	});

	it("requires run identity and keeps multiple runs in one stream distinct", async () => {
		const lifecycle = event_fixtures["run.lifecycle"].payload;
		const usage = event_fixtures["run.usage.updated"].payload;
		const missing_run = [
			event_envelope(lifecycle, { run_id: false, stream_id: "shared_stream" }),
			event_envelope(usage, {
				journal_sequence: 2,
				run_id: false,
				stream_id: "shared_stream",
			}),
		];

		await expect(Effect.runPromise(Project(missing_run[0]))).resolves.toEqual([]);
		await expect(Effect.runPromise(Project(missing_run[1]))).resolves.toEqual([]);
		await expect(Effect.runPromise(ProjectMany(missing_run))).resolves.toEqual([]);

		const projected = await Effect.runPromise(
			ProjectMany([
				event_envelope(lifecycle, {
					journal_sequence: 3,
					run_id: "run_same_stream_a",
					stream_id: "shared_stream",
				}),
				event_envelope(usage, {
					journal_sequence: 4,
					run_id: "run_same_stream_b",
					stream_id: "shared_stream",
				}),
			]),
		);

		expect(projected.map((item) => item.surface_id)).toEqual([
			"surface:run:run_same_stream_a",
			"surface:run:run_same_stream_b",
		]);
	});

	it("reconciles run usage with lifecycle metadata in either order", async () => {
		const lifecycle = event_fixtures["run.lifecycle"].payload;
		const usage = event_fixtures["run.usage.updated"].payload;
		const expected = expected_surface({
			group: "Work",
			kind: "run",
			label: "Run",
			state: "running",
			summary: "Run updated.",
			surface_id: "surface:run:run_envelope",
			usage: usage.usage,
		});
		const [usage_last, lifecycle_last] = await Promise.all([
			Effect.runPromise(
				ProjectMany([
					event_envelope(lifecycle, { journal_sequence: 5 }),
					event_envelope(usage, { journal_sequence: 6 }),
				]),
			),
			Effect.runPromise(
				ProjectMany([
					event_envelope(usage, { journal_sequence: 7 }),
					event_envelope(lifecycle, { journal_sequence: 8 }),
				]),
			),
		]);

		expect(usage_last).toEqual([expected]);
		expect(lifecycle_last).toEqual([expected]);
	});

	it("keeps completed lifecycle metadata when usage arrives last", async () => {
		const usage = event_fixtures["run.usage.updated"].payload;
		const completed = {
			...event_fixtures["run.lifecycle"].payload,
			state: "completed" as const,
		};
		const projected = await Effect.runPromise(
			ProjectMany([
				event_envelope(completed, {
					agent_id: "agent_lifecycle",
					journal_sequence: 9,
				}),
				event_envelope(usage, {
					agent_id: "agent_usage",
					journal_sequence: 10,
				}),
			]),
		);

		expect(projected).toEqual([
			expected_surface({
				agent_id: "agent_lifecycle",
				group: "Work",
				kind: "run",
				label: "Run",
				state: "completed",
				summary: "Run updated.",
				surface_id: "surface:run:run_envelope",
				usage: usage.usage,
			}),
		]);
	});

	it("upserts paired capability metadata without changing first-seen order", async () => {
		const paired = await Effect.runPromise(
			ProjectMany([
				event_envelope(event_fixtures["capability.invocation.updated"].payload, {
					journal_sequence: 8,
				}),
				event_envelope(event_fixtures["tool.invocation.updated"].payload, {
					journal_sequence: 9,
				}),
			]),
		);
		const projected = await Effect.runPromise(
			ProjectMany([
				event_envelope(event_fixtures["thread.created"].payload, {
					journal_sequence: 10,
				}),
				event_envelope(event_fixtures["capability.invocation.updated"].payload, {
					journal_sequence: 11,
				}),
				event_envelope(event_fixtures["process.ownership"].payload, {
					journal_sequence: 12,
				}),
				event_envelope(event_fixtures["tool.invocation.updated"].payload, {
					journal_sequence: 13,
				}),
			]),
		);

		expect(paired).toHaveLength(1);
		expect(paired[0]).toMatchObject({
			label: "Read workspace metadata",
			source: "marketplace",
			surface_id: "surface:capability:invocation_shared",
			summary: "Reads curated workspace metadata.",
		});
		expect(projected.map((item) => item.surface_id)).toEqual([
			"surface:thread:thread_1",
			"surface:capability:invocation_shared",
			"surface:process:ownership:cause_1",
		]);
		expect(projected[1]).toMatchObject({
			agent_id: "agent_tool",
			label: "Read workspace metadata",
			run_id: "run_tool",
			source: "marketplace",
			summary: "Reads curated workspace metadata.",
			workspace_id: "workspace_tool",
		});
	});

	it("replaces lifecycle updates while retaining full tool metadata", async () => {
		const completed_run = {
			...event_fixtures["run.lifecycle"].payload,
			state: "completed" as const,
		};
		const later_capability = {
			...event_fixtures["capability.invocation.updated"].payload,
			label: "Generic capability activity",
			state: "failed" as const,
			summary: "Generic capability summary.",
		};
		const projected = await Effect.runPromise(
			ProjectMany([
				event_envelope(event_fixtures["run.lifecycle"].payload, {
					journal_sequence: 20,
				}),
				event_envelope(event_fixtures["run.usage.updated"].payload, {
					journal_sequence: 21,
				}),
				event_envelope(completed_run, { journal_sequence: 22 }),
				event_envelope(event_fixtures["tool.invocation.updated"].payload, {
					journal_sequence: 23,
				}),
				event_envelope(later_capability, { journal_sequence: 24 }),
			]),
		);
		const run = projected.find((item) => item.surface_id === "surface:run:run_envelope");
		const capability = projected.find(
			(item) => item.surface_id === "surface:capability:invocation_shared",
		);

		expect(projected).toHaveLength(2);
		expect(
			projected.filter((item) => item.surface_id === "surface:run:run_envelope"),
		).toHaveLength(1);
		expect(run).toMatchObject({
			state: "completed",
			summary: "Run updated.",
			usage: { input_tokens: 1_024, output_tokens: 256 },
		});
		expect(capability).toMatchObject({
			label: "Read workspace metadata",
			source: "marketplace",
			state: "failed",
			summary: "Reads curated workspace metadata.",
		});
	});

	it("uses only envelope raw origin and omits private payload fields", async () => {
		const projected = await Effect.runPromise(
			Project(
				event_envelope(event_fixtures["artifact.recorded"].payload, {
					include_raw_origin: false,
				}),
			),
		);
		const serialized = JSON.stringify(projected);

		expect(projected[0]).not.toHaveProperty("raw_origin");
		expect(serialized).not.toContain("PRIVATE_ARTIFACT_CONTENT");
		expect(serialized).not.toContain("payload_artifact_origin");
		expect(serialized).not.toContain("Private provider summary label");
	});

	it("fails closed for future variants, private fields, and unsafe raw origin", async () => {
		const capability = event_fixtures["capability.invocation.updated"].payload;
		const tool = event_fixtures["tool.invocation.updated"].payload;
		const valid = event_envelope(capability);
		const invalid = [
			{ ...valid, payload: { type: "engine.future_action" } },
			{ ...valid, payload: { ...capability, arguments: { token: "private" } } },
			{ ...valid, payload: { ...capability, diagnostics: "private" } },
			{ ...valid, payload: { ...capability, results: { output: "private" } } },
			{ ...valid, payload: { ...capability, secrets: "private" } },
			{
				...event_envelope(tool),
				payload: {
					...tool,
					invocation: { ...tool.invocation, result: { private: true } },
				},
			},
			{
				...valid,
				raw_origin: {
					metadata: { private: true },
					provider: "engine_1",
					reference: "opaque_1",
				},
			},
			{
				...valid,
				raw_origin: {
					provider: "engine_1",
					reference: "x".repeat(surface_raw_origin_identifier_maximum_bytes + 1),
				},
			},
		];

		for (const input of invalid) {
			await expect(Effect.runPromise(Project(input))).rejects.toThrow();
		}

		await expect(Effect.runPromise(ProjectMany([valid, invalid[0]]))).rejects.toThrow();
	});
});
