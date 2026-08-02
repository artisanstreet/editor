export type DevLaneId = "build" | "forge" | "runner" | "web";

export type DevLaneStatus = "failed" | "ready" | "running" | "starting" | "stopped" | "waiting";

export interface DevLaneDefinition {
	readonly id: DevLaneId;
	readonly label: string;
	readonly status: DevLaneStatus;
}

export interface DevTuiLane extends DevLaneDefinition {
	readonly log_lines: ReadonlyArray<string>;
}

export interface DevTuiState {
	readonly forge_origin: string | undefined;
	readonly lanes: ReadonlyArray<DevTuiLane>;
	readonly max_log_lines: number;
	readonly selected_lane_id: DevLaneId;
	readonly title: string;
	readonly web_origin: string | undefined;
}

export type DevTuiEvent =
	| {
			readonly forge_origin?: string;
			readonly lanes: ReadonlyArray<DevLaneDefinition>;
			readonly title: string;
			readonly type: "configure";
			readonly web_origin?: string;
	  }
	| {
			readonly lane_id: DevLaneId;
			readonly line: string;
			readonly type: "log";
	  }
	| {
			readonly lane_id: DevLaneId;
			readonly status: DevLaneStatus;
			readonly type: "status";
	  }
	| {
			readonly type: "shutdown";
	  };

const default_lane: DevLaneDefinition = {
	id: "runner",
	label: "Overview",
	status: "starting",
};

const dev_lane_ids = new Set<DevLaneId>(["build", "forge", "runner", "web"]);
const dev_lane_statuses = new Set<DevLaneStatus>([
	"failed",
	"ready",
	"running",
	"starting",
	"stopped",
	"waiting",
]);

export const create_dev_tui_state = (
	lanes: ReadonlyArray<DevLaneDefinition> = [default_lane],
	max_log_lines = 1_000,
): DevTuiState => {
	const configured_lanes = lanes.length === 0 ? [default_lane] : lanes;

	return {
		forge_origin: undefined,
		lanes: configured_lanes.map((lane) => ({ ...lane, log_lines: [] })),
		max_log_lines,
		selected_lane_id: configured_lanes[0]?.id ?? default_lane.id,
		title: "Artisan development",
		web_origin: undefined,
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
			forge_origin: event.forge_origin,
			lanes: configured_lanes.map((lane) => ({
				...lane,
				log_lines: previous_lanes.get(lane.id)?.log_lines ?? [],
			})),
			selected_lane_id,
			title: event.title,
			web_origin: event.web_origin,
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
	const next_index = (current_index + delta + lane_count) % lane_count;
	const next_lane = state.lanes[next_index];

	if (next_lane === undefined) return state;

	return select_dev_tui_lane(state, next_lane.id);
};

const is_record = (value: unknown): value is Readonly<Record<string, unknown>> =>
	typeof value === "object" && value !== null;

const is_lane_id = (value: unknown): value is DevLaneId =>
	typeof value === "string" && dev_lane_ids.has(value as DevLaneId);

const is_lane_status = (value: unknown): value is DevLaneStatus =>
	typeof value === "string" && dev_lane_statuses.has(value as DevLaneStatus);

const is_lane_definition = (value: unknown): value is DevLaneDefinition =>
	is_record(value) &&
	is_lane_id(value.id) &&
	typeof value.label === "string" &&
	is_lane_status(value.status);

export const is_dev_tui_event = (value: unknown): value is DevTuiEvent => {
	if (!is_record(value) || typeof value.type !== "string") return false;

	if (value.type === "shutdown") return true;

	if (value.type === "log") {
		return is_lane_id(value.lane_id) && typeof value.line === "string";
	}

	if (value.type === "status") {
		return is_lane_id(value.lane_id) && is_lane_status(value.status);
	}

	return (
		value.type === "configure" &&
		typeof value.title === "string" &&
		Array.isArray(value.lanes) &&
		value.lanes.every(is_lane_definition) &&
		(value.forge_origin === undefined || typeof value.forge_origin === "string") &&
		(value.web_origin === undefined || typeof value.web_origin === "string")
	);
};
