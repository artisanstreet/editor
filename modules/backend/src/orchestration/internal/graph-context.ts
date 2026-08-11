import { Context, Effect } from "effect";

import type { EngineRunTerminalState } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import { RuntimeMetadata } from "../../runtime/metadata";
import type { AgentNameCatalog } from "../agent-name-catalog";
import {
	AgentGraphInvalid,
	type AgentGraphCommand,
	type AgentGraphControlAction,
} from "../agent-graph-model";

export type GraphDatabase = Context.Service.Shape<typeof Database>;
export type GraphTransaction = GraphDatabase["client"];
export type GraphMetadata = Context.Service.Shape<typeof RuntimeMetadata>;
export type GraphNotifier = Context.Service.Shape<typeof JournalNotifier>;

export interface GraphContext {
	readonly agent_name_catalog: Context.Service.Shape<typeof AgentNameCatalog>;
	readonly database: GraphDatabase;
	readonly metadata: GraphMetadata;
	readonly notifier: GraphNotifier;
}

export interface GraphTransitionInput {
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly group_id: string;
	readonly thread_id: string;
}

const terminal_states = ["complete", "failed", "stopped"] as const;
const unsafe_status_fragments = [
	"chain of thought",
	"chain-of-thought",
	"hidden reasoning",
	"internal reasoning",
	"private reasoning",
	"step-by-step reasoning",
	"<thinking>",
] as const;

export function is_terminal_state(state: string): state is (typeof terminal_states)[number] {
	return terminal_states.some((terminal) => terminal === state);
}

export function terminal_state_from_engine(
	state: EngineRunTerminalState,
): "complete" | "failed" | "stopped" {
	return state === "completed" ? "complete" : state === "failed" ? "failed" : "stopped";
}

export function command_matches(
	command: CommandEnvelope,
	existing: {
		readonly agent_id: string | null;
		readonly causation_id: string | null;
		readonly origin: string;
		readonly payload_json: string;
		readonly raw_origin_json: string | null;
		readonly run_id: string | null;
		readonly schema_version: number;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) {
	return (
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.payload_json === JSON.stringify(command.payload) &&
		existing.raw_origin_json ===
			(command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
		existing.run_id === (command.run_id ?? null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at &&
		existing.thread_id === command.thread_id
	);
}

export function graph_identity(payload: AgentGraphCommand) {
	return {
		group_id: payload.group_id,
		...(payload.type === "orchestration.group.start" || payload.type === "agent_instance.rename"
			? {}
			: { assignment_id: payload.assignment_id }),
	};
}

export function control_action(payload: AgentGraphCommand): AgentGraphControlAction | undefined {
	return payload.type === "assignment.steer"
		? "steer"
		: payload.type === "assignment.stop"
			? "stop"
			: payload.type === "assignment.pause"
				? "pause"
				: payload.type === "assignment.resume"
					? "resume"
					: undefined;
}

export function title_case_role(role: string) {
	const first_character = role.at(0);
	return first_character === undefined
		? "Agent"
		: `${first_character.toUpperCase()}${role.slice(1)}`;
}

export const visible_name_maximum = 64;

const has_control_character = (value: string) =>
	[...value].some((character) => {
		const code = character.codePointAt(0);

		return code !== undefined && (code <= 31 || code === 127 || (code >= 128 && code <= 159));
	});

export function normalize_visible_label(
	value: string,
	field: string,
	maximum = visible_name_maximum,
) {
	const compact = value.trim().replace(/\s+/g, " ");

	if (value.length > 256) {
		return Effect.fail(
			new AgentGraphInvalid({ message: `${field} must not exceed 256 input characters` }),
		);
	}

	if (has_control_character(value)) {
		return Effect.fail(
			new AgentGraphInvalid({ message: `${field} must not contain control characters` }),
		);
	}

	if (compact.length === 0 || compact.length > maximum) {
		return Effect.fail(
			new AgentGraphInvalid({
				message: `${field} must contain between 1 and ${maximum} visible characters`,
			}),
		);
	}

	return Effect.succeed(compact);
}

export function compact_status_text(value: string, field: string, maximum: number) {
	const compact = value.trim().replace(/\s+/g, " ");
	const normalized = compact.toLowerCase();

	if (value.length > 8192 || has_control_character(value) || compact.length > 4096) {
		return Effect.fail(
			new AgentGraphInvalid({
				message: `${field} must contain compact visible status text`,
			}),
		);
	}

	if (unsafe_status_fragments.some((fragment) => normalized.includes(fragment))) {
		return Effect.fail(
			new AgentGraphInvalid({
				message: `${field} must describe observable work without private reasoning`,
			}),
		);
	}

	if (compact.length === 0) {
		return Effect.fail(new AgentGraphInvalid({ message: `${field} must not be empty` }));
	}

	return Effect.succeed(compact.slice(0, maximum));
}
