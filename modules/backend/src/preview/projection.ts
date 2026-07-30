import { Schema } from "effect";

import {
	PreviewTargetHealth,
	type PreviewInspectionSession,
	type PreviewTarget,
} from "@artisan/protocol";

import {
	type PreviewInspectionProjection,
	PreviewRoutes,
	type PreviewTargetProjection,
} from "./contracts";

const DecodeHealth = Schema.decodeUnknownSync(Schema.fromJsonString(PreviewTargetHealth));
const DecodeRoutes = Schema.decodeUnknownSync(Schema.fromJsonString(PreviewRoutes));

export function to_target(value: PreviewTargetProjection): PreviewTarget {
	let health: PreviewTarget["health"] | undefined;
	try {
		health = value.health_json === null ? undefined : DecodeHealth(value.health_json);
	} catch {
		health = undefined;
	}
	let routes: ReadonlyArray<string> = [];
	try {
		routes = DecodeRoutes(value.routes_json);
	} catch {
		routes = [];
	}
	return {
		created_at: value.created_at,
		...(health === undefined ? {} : { health }),
		id: value.target_id,
		journal_sequence: value.journal_sequence,
		...(value.last_error === null ? {} : { last_error: value.last_error }),
		launch_state: value.launch_state,
		port: value.port,
		project_id: value.project_id,
		routes,
		...(value.source === undefined ? {} : { source: value.source }),
		state: value.state === "removed" ? "stopped" : value.state,
		thread_id: value.thread_id,
		updated_at: value.updated_at,
		url: value.url,
		workspace_id: value.workspace_id,
	};
}

export function to_session(value: PreviewInspectionProjection): PreviewInspectionSession {
	return {
		...(value.closed_at === null ? {} : { closed_at: value.closed_at }),
		connector_id: value.connector_id,
		...(value.last_error === null ? {} : { last_error: value.last_error }),
		opened_at: value.opened_at,
		reconnect_state: value.reconnect_state,
		session_id: value.session_id,
		state: value.state,
		target_id: value.target_id,
		updated_at: value.updated_at,
	};
}
