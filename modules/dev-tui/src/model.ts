import { Schema } from "effect";

import strip_ansi from "strip-ansi";

export const DevLaneId = Schema.NonEmptyString;
export type DevLaneId = typeof DevLaneId.Type;

export const DevLaneStatus = Schema.Literals([
	"failed",
	"ready",
	"running",
	"starting",
	"stopped",
	"waiting",
]);
export type DevLaneStatus = typeof DevLaneStatus.Type;

export const DevEndpoint = Schema.Struct({
	label: Schema.NonEmptyString,
	url: Schema.NonEmptyString,
});
export type DevEndpoint = typeof DevEndpoint.Type;

export const DevLaneDefinition = Schema.Struct({
	id: DevLaneId,
	label: Schema.NonEmptyString,
	status: DevLaneStatus,
});
export type DevLaneDefinition = typeof DevLaneDefinition.Type;

export interface DevTuiLane extends DevLaneDefinition {
	readonly log_lines: ReadonlyArray<string>;
}

export interface DevTuiState {
	readonly endpoints: ReadonlyArray<DevEndpoint>;
	readonly lanes: ReadonlyArray<DevTuiLane>;
	readonly max_log_lines: number;
	readonly selected_lane_id: DevLaneId;
	readonly title: string;
}

export const DevTuiEvent = Schema.Union([
	Schema.Struct({
		endpoints: Schema.Array(DevEndpoint),
		lanes: Schema.Array(DevLaneDefinition),
		title: Schema.NonEmptyString,
		type: Schema.Literal("configure"),
	}),
	Schema.Struct({
		lane_id: DevLaneId,
		line: Schema.String,
		type: Schema.Literal("log"),
	}),
	Schema.Struct({
		lane_id: DevLaneId,
		status: DevLaneStatus,
		type: Schema.Literal("status"),
	}),
	Schema.Struct({ type: Schema.Literal("shutdown") }),
]).pipe(Schema.toTaggedUnion("type"));
export type DevTuiEvent = typeof DevTuiEvent.Type;

const default_lane: DevLaneDefinition = {
	id: "runner",
	label: "Overview",
	status: "starting",
};

export const create_dev_tui_state = (
	lanes: ReadonlyArray<DevLaneDefinition> = [default_lane],
	max_log_lines = 1_000,
): DevTuiState => {
	const configured_lanes = lanes.length === 0 ? [default_lane] : lanes;
	const bounded_log_lines = Number.isFinite(max_log_lines)
		? Math.max(1, Math.floor(max_log_lines))
		: 1_000;

	return {
		endpoints: [],
		lanes: configured_lanes.map((lane) => ({ ...lane, log_lines: [] })),
		max_log_lines: bounded_log_lines,
		selected_lane_id: configured_lanes[0]?.id ?? default_lane.id,
		title: "Development",
	};
};

const replace_lane = (
	state: DevTuiState,
	lane_id: DevLaneId,
	update: (lane: DevTuiLane) => DevTuiLane,
): DevTuiState => {
	const lane_exists = state.lanes.some((lane) => lane.id === lane_id);

	if (!lane_exists) return state;

	return {
		...state,
		lanes: state.lanes.map((lane) => (lane.id === lane_id ? update(lane) : lane)),
	};
};

export const apply_dev_tui_event = (state: DevTuiState, event: DevTuiEvent): DevTuiState => {
	if (event.type === "shutdown") return state;

	if (event.type === "configure") {
		const previous_lanes = new Map(state.lanes.map((lane) => [lane.id, lane]));
		const configured_lanes = event.lanes.length === 0 ? [default_lane] : event.lanes;
		const selected_lane_id = configured_lanes.some((lane) => lane.id === state.selected_lane_id)
			? state.selected_lane_id
			: (configured_lanes[0]?.id ?? default_lane.id);

		return {
			...state,
			endpoints: event.endpoints,
			lanes: configured_lanes.map((lane) => ({
				...lane,
				log_lines: previous_lanes.get(lane.id)?.log_lines ?? [],
			})),
			selected_lane_id,
			title: event.title,
		};
	}

	if (event.type === "status") {
		return replace_lane(state, event.lane_id, (lane) => ({
			...lane,
			status: event.status,
		}));
	}

	return replace_lane(state, event.lane_id, (lane) => ({
		...lane,
		log_lines: [...lane.log_lines, event.line].slice(-state.max_log_lines),
	}));
};

export const select_dev_tui_lane = (state: DevTuiState, lane_id: DevLaneId): DevTuiState => {
	const lane_exists = state.lanes.some((lane) => lane.id === lane_id);

	if (!lane_exists || lane_id === state.selected_lane_id) return state;

	return { ...state, selected_lane_id: lane_id };
};

export const select_relative_dev_tui_lane = (state: DevTuiState, delta: number): DevTuiState => {
	const current_index = state.lanes.findIndex((lane) => lane.id === state.selected_lane_id);
	const lane_count = state.lanes.length;

	if (lane_count === 0) return state;

	const next_index = (((current_index + delta) % lane_count) + lane_count) % lane_count;
	const next_lane = state.lanes[next_index];

	if (next_lane === undefined) return state;

	return select_dev_tui_lane(state, next_lane.id);
};

/** Removes terminal formatting that must not be retained in dashboard log state. */
export const sanitize_dev_log_line = (line: string): string =>
	strip_ansi(line).replaceAll("\r", "");

export const is_dev_tui_event = Schema.is(DevTuiEvent);
