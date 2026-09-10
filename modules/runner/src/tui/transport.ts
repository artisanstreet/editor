import { Schema } from "effect";

export const DashboardStatus = Schema.Literals([
	"failed",
	"ready",
	"running",
	"starting",
	"stopped",
	"waiting",
]);

export const DashboardConfiguration = Schema.Struct({
	endpoints: Schema.Array(
		Schema.Struct({
			label: Schema.String,
			url: Schema.String,
		}),
	),
	lanes: Schema.Array(
		Schema.Struct({
			id: Schema.String,
			name: Schema.String,
			status: Schema.optional(DashboardStatus),
		}),
	),
	max_log_lines: Schema.Number,
	title: Schema.String,
});
export type DashboardConfiguration = typeof DashboardConfiguration.Type;

export const DashboardEvent = Schema.Union([
	Schema.Struct({
		lane_id: Schema.String,
		line: Schema.String,
		type: Schema.Literal("log"),
	}),
	Schema.Struct({
		lane_id: Schema.String,
		status: DashboardStatus,
		type: Schema.Literal("status"),
	}),
	Schema.Struct({ type: Schema.Literal("shutdown") }),
]).pipe(Schema.toTaggedUnion("type"));
export type DashboardEvent = typeof DashboardEvent.Type;

export const DashboardCommand = Schema.Union([
	Schema.Struct({ type: Schema.Literal("ready") }),
	Schema.Struct({ type: Schema.Literal("shutdown") }),
]).pipe(Schema.toTaggedUnion("type"));
export type DashboardCommand = typeof DashboardCommand.Type;

export const DecodeDashboardConfiguration = Schema.decodeUnknownSync(
	Schema.fromJsonString(DashboardConfiguration),
);
export const DecodeDashboardEvent = Schema.decodeUnknownSync(Schema.fromJsonString(DashboardEvent));
export const DecodeDashboardCommand = Schema.decodeUnknownSync(
	Schema.fromJsonString(DashboardCommand),
);
