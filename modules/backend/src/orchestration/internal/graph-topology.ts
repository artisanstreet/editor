import { Effect } from "effect";

import type { AssignmentSpec } from "@artisan/protocol";

import { AgentGraphInvalid, type AgentGraphCommand } from "../agent-graph-model";
import {
	default_agent_name_bank,
	normalize_visible_label,
	title_case_role,
	visible_name_maximum,
	type GraphContext,
} from "./graph-context";

type StartGroupCommand = Extract<AgentGraphCommand, { readonly type: "orchestration.group.start" }>;

export interface AllocatedAgentInstances {
	readonly assignment_agents: ReadonlyMap<string, string>;
	readonly assignment_roles: ReadonlyMap<string, string>;
	readonly instances: ReadonlyArray<{
		readonly agent_id: string;
		readonly created_at: string;
		readonly display_name: string;
		readonly group_id: string;
		readonly role: string;
		readonly updated_at: string;
	}>;
}

export interface GraphTopology {
	readonly allocate_agent_instances: (
		assignments: ReadonlyArray<AssignmentSpec>,
		group_id: string,
		coordinator_agent_id: string,
		name_bank: ReadonlyArray<string>,
		created_at: string,
	) => Effect.Effect<AllocatedAgentInstances, AgentGraphInvalid>;
	readonly validate_topology: (
		payload: StartGroupCommand,
	) => Effect.Effect<void, AgentGraphInvalid>;
	readonly default_name_bank: ReadonlyArray<string>;
}

/** Owns graph topology validation and deterministic visible identity allocation. */
export function make_graph_topology(context: GraphContext): GraphTopology {
	const { metadata } = context;

	const validate_topology = (payload: StartGroupCommand) =>
		Effect.gen(function* () {
			const assignment_ids = payload.assignments.map(({ assignment_id }) => assignment_id);
			const join_ids = (payload.joins ?? []).map(({ join_id }) => join_id);
			const edge_ids = (payload.edges ?? []).map(({ edge_id }) => edge_id);
			const all_nodes = new Set([payload.group_id, ...assignment_ids, ...join_ids]);

			if (new Set(assignment_ids).size !== assignment_ids.length) {
				return yield* new AgentGraphInvalid({
					message: "Assignment identifiers must be unique within a group",
				});
			}

			if (new Set(join_ids).size !== join_ids.length) {
				return yield* new AgentGraphInvalid({
					message: "Join identifiers must be unique within a group",
				});
			}

			if (new Set(edge_ids).size !== edge_ids.length) {
				return yield* new AgentGraphInvalid({
					message: "Graph edge identifiers must be unique within a group",
				});
			}

			for (const [index, name] of (payload.name_bank ?? []).entries()) {
				yield* normalize_visible_label(name, `Agent name bank entry ${index + 1}`);
			}

			for (const assignment of payload.assignments) {
				yield* normalize_visible_label(
					assignment.role,
					`Assignment ${assignment.assignment_id} role`,
				);

				if (assignment.display_name !== undefined) {
					yield* normalize_visible_label(
						assignment.display_name,
						`Assignment ${assignment.assignment_id} display name`,
					);
				}

				if (!all_nodes.has(assignment.parent_node_id)) {
					return yield* new AgentGraphInvalid({
						message: `Assignment ${assignment.assignment_id} has an unknown parent node`,
					});
				}
			}

			for (const join of payload.joins ?? []) {
				if (
					new Set(join.upstream_assignment_ids).size !==
					join.upstream_assignment_ids.length
				) {
					return yield* new AgentGraphInvalid({
						message: `Join ${join.join_id} upstream assignments must be unique`,
					});
				}

				if (
					join.upstream_assignment_ids.some(
						(assignment_id) => !assignment_ids.includes(assignment_id),
					)
				) {
					return yield* new AgentGraphInvalid({
						message: `Join ${join.join_id} has an unknown upstream assignment`,
					});
				}

				if (
					(join.strategy === "synthesize" || join.strategy === "review") &&
					!join.downstream_assignment_id
				) {
					return yield* new AgentGraphInvalid({
						message: `Join ${join.join_id} requires an explicit downstream assignment`,
					});
				}

				if (
					join.downstream_assignment_id &&
					!assignment_ids.includes(join.downstream_assignment_id)
				) {
					return yield* new AgentGraphInvalid({
						message: `Join ${join.join_id} has an unknown downstream assignment`,
					});
				}

				const downstream = payload.assignments.find(
					({ assignment_id }) => assignment_id === join.downstream_assignment_id,
				);

				if (downstream && downstream.parent_node_id !== join.join_id) {
					return yield* new AgentGraphInvalid({
						message: `Downstream assignment ${downstream.assignment_id} must name join ${join.join_id} as its parent`,
					});
				}
			}

			for (const edge of payload.edges ?? []) {
				if (!all_nodes.has(edge.from_node_id) || !all_nodes.has(edge.to_node_id)) {
					return yield* new AgentGraphInvalid({
						message: `Graph edge ${edge.edge_id} references an unknown node`,
					});
				}

				if (
					edge.kind === "dependency" &&
					(!assignment_ids.includes(edge.from_node_id) ||
						!assignment_ids.includes(edge.to_node_id))
				) {
					return yield* new AgentGraphInvalid({
						message: `Dependency edge ${edge.edge_id} must connect two assignments`,
					});
				}

				if (edge.kind === "dependency" && edge.from_node_id === edge.to_node_id) {
					return yield* new AgentGraphInvalid({
						message: `Dependency edge ${edge.edge_id} cannot depend on itself`,
					});
				}
			}

			const explicit_dependencies = (payload.edges ?? []).filter(
				({ kind }) => kind === "dependency",
			);
			const dependency_pairs = explicit_dependencies.map(
				({ from_node_id, to_node_id }) => `${from_node_id}\u0000${to_node_id}`,
			);

			if (new Set(dependency_pairs).size !== dependency_pairs.length) {
				return yield* new AgentGraphInvalid({
					message: "Dependency edges must not repeat the same assignment relationship",
				});
			}

			const semantic_dependencies = [
				...explicit_dependencies,
				...(payload.joins ?? []).flatMap((join) => {
					const downstream_assignment_id = join.downstream_assignment_id;
					return downstream_assignment_id === undefined
						? []
						: join.upstream_assignment_ids.map((from_node_id) => ({
								from_node_id,
								to_node_id: downstream_assignment_id,
							}));
				}),
			];
			const adjacency = new Map(
				assignment_ids.map((assignment_id) => [assignment_id, [] as string[]]),
			);

			for (const dependency of semantic_dependencies) {
				const downstream = adjacency.get(dependency.from_node_id);
				if (downstream === undefined)
					return yield* new AgentGraphInvalid({
						message: `Dependency references unknown assignment ${dependency.from_node_id}`,
					});
				downstream.push(dependency.to_node_id);
			}

			const visiting = new Set<string>();
			const visited = new Set<string>();
			const has_cycle = (assignment_id: string): boolean => {
				if (visiting.has(assignment_id)) {
					return true;
				}

				if (visited.has(assignment_id)) {
					return false;
				}

				visiting.add(assignment_id);

				if ((adjacency.get(assignment_id) ?? []).some(has_cycle)) {
					return true;
				}

				visiting.delete(assignment_id);
				visited.add(assignment_id);

				return false;
			};

			if (assignment_ids.some(has_cycle)) {
				return yield* new AgentGraphInvalid({
					message: "Dependency topology must be acyclic",
				});
			}
		});

	const allocate_agent_instances = (
		assignments: ReadonlyArray<AssignmentSpec>,
		group_id: string,
		coordinator_agent_id: string,
		name_bank: ReadonlyArray<string>,
		created_at: string,
	) =>
		Effect.gen(function* () {
			const used_names = new Set(["coordinator"]);
			const instances = new Map<string, AllocatedAgentInstances["instances"][number]>();
			const assignment_agents = new Map<string, string>();
			const assignment_roles = new Map<string, string>();
			const role_counts = new Map<string, number>();

			instances.set(coordinator_agent_id, {
				agent_id: coordinator_agent_id,
				created_at,
				display_name: "Coordinator",
				group_id,
				role: "coordinator",
				updated_at: created_at,
			});

			for (const [index, assignment] of assignments.entries()) {
				const agent_id = assignment.agent_id ?? (yield* metadata.MakeId("agent"));
				const normalized_role = yield* normalize_visible_label(
					assignment.role,
					`Assignment ${assignment.assignment_id} role`,
				);

				assignment_agents.set(assignment.assignment_id, agent_id);
				assignment_roles.set(assignment.assignment_id, normalized_role);

				if (instances.has(agent_id)) {
					continue;
				}

				const role_name = title_case_role(normalized_role);
				const role_count = (role_counts.get(role_name) ?? 1) + 1;
				const preferred = yield* normalize_visible_label(
					assignment.display_name ?? name_bank[index] ?? `${role_name} ${role_count}`,
					`Assignment ${assignment.assignment_id} display name`,
				);
				let display_name = preferred;
				let suffix = 2;

				while (used_names.has(display_name.toLowerCase())) {
					const suffix_text = ` ${suffix}`;

					display_name = `${preferred.slice(0, visible_name_maximum - suffix_text.length)}${suffix_text}`;
					suffix += 1;
				}

				role_counts.set(role_name, role_count);
				used_names.add(display_name.toLowerCase());
				instances.set(agent_id, {
					agent_id,
					created_at,
					display_name,
					group_id,
					role: normalized_role,
					updated_at: created_at,
				});
			}

			return { assignment_agents, assignment_roles, instances: [...instances.values()] };
		});

	return {
		allocate_agent_instances,
		validate_topology,
		default_name_bank: default_agent_name_bank,
	};
}
