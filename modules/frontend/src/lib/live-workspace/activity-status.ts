import { Option } from "effect";

import type { SurfaceItem } from "@artisan/protocol";

import type { LiveWorkspaceSnapshot } from "./store";

export const PlayfulActivityLabels = [
	"Pondering",
	"Percolating",
	"Recombobulating",
	"Puttering",
	"Zesting",
] as const;

const ObservableSurfaceLabel = (item: SurfaceItem | undefined): string | undefined => {
	if (item === undefined) return undefined;
	if (item.category === "native_action" && item.kind === "compaction") {
		return "Compacting context";
	}
	if (item.category === "permission" && item.kind === "approval") {
		return "Waiting for approval";
	}
	if (
		item.category === "process" ||
		item.category === "change" ||
		item.category === "capability" ||
		item.category === "routine"
	) {
		return item.summary.status ?? item.summary.label;
	}
	return undefined;
};

export interface ActivityStatusView {
	readonly active: boolean;
	readonly label: string;
	readonly mode: "idle" | "working" | "compacting" | "waiting";
}

const ActiveOrchestrationStates = new Set(["queued", "running", "waiting", "joining"]);

/** Reduces public projections to the single native desktop working signal. */
export const HasActiveWorkspaceWork = (snapshot: LiveWorkspaceSnapshot): boolean => {
	const selected_work = Option.getOrUndefined(snapshot.thread_work);
	if (selected_work?.status === "running" || selected_work?.status === "waiting") return true;

	const groups = Option.getOrUndefined(snapshot.orchestration_groups)?.groups ?? [];
	if (groups.some((group) => ActiveOrchestrationStates.has(group.state))) return true;

	return snapshot.threads.some((thread) => thread.live_status.toLowerCase() === "working");
};

/** Selects a safe visible activity label without exposing provider reasoning text. */
export const MakeActivityStatusView = (
	snapshot: LiveWorkspaceSnapshot,
	phrase_index: number,
	reduced_motion: boolean,
): ActivityStatusView => {
	const run = Option.getOrUndefined(snapshot.thread_work);
	if (run?.status !== "running" && run?.status !== "waiting") {
		return { active: false, label: "Idle", mode: "idle" };
	}

	const pending_question = Option.getOrUndefined(snapshot.session)?.pending_question;
	if (pending_question?.state === "pending") {
		return { active: true, label: "Waiting for your reply", mode: "waiting" };
	}

	const latest_surface = [...(Option.getOrUndefined(snapshot.surface_items)?.items ?? [])]
		.reverse()
		.find(
			(item) =>
				item.attribution.run_id === undefined || item.attribution.run_id === run.run_id,
		);
	const observable_label = ObservableSurfaceLabel(latest_surface);
	if (observable_label !== undefined) {
		return {
			active: true,
			label: observable_label,
			mode:
				latest_surface?.category === "native_action" && latest_surface.kind === "compaction"
					? "compacting"
					: run.status === "waiting"
						? "waiting"
						: "working",
		};
	}

	if (reduced_motion) {
		return {
			active: true,
			label: run.status === "waiting" ? "Waiting" : "Working",
			mode: run.status === "waiting" ? "waiting" : "working",
		};
	}

	return {
		active: true,
		label: PlayfulActivityLabels[phrase_index % PlayfulActivityLabels.length] ?? "Working",
		mode: run.status === "waiting" ? "waiting" : "working",
	};
};
