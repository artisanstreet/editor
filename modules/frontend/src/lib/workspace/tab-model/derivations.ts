import { Effect, Option } from "effect";

import { FindTabIndex } from "./internal";
import type { TabOverflow, WorkspaceFileReference, WorkspaceState, WorkspaceTab } from "./types";

export const DeriveTabOverflow = (state: WorkspaceState, max_visible: number) =>
	Effect.gen(function* () {
		const limit = Math.max(1, Math.trunc(max_visible));
		if (state.tabs.length <= limit)
			return { visible: state.tabs, overflow: [] } satisfies TabOverflow;

		const visible = state.tabs.slice(0, limit);
		const active_tab_id = state.active_tab_id;
		if (
			Option.isSome(active_tab_id) &&
			!visible.some((tab) => tab.id === active_tab_id.value)
		) {
			const active_index = yield* FindTabIndex(state.tabs, active_tab_id.value);
			if (Option.isSome(active_index)) {
				const active_tab = state.tabs.at(active_index.value);
				if (active_tab !== undefined) {
					visible.pop();
					visible.unshift(active_tab);
				}
			}
		}

		const visible_ids = new Set(visible.map((tab) => tab.id));
		const overflow: Array<WorkspaceTab> = state.tabs.filter((tab) => !visible_ids.has(tab.id));
		return { visible, overflow } satisfies TabOverflow;
	});

export const DeriveBreadcrumbs = (file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		return file.path.split("/").filter((segment) => segment.length > 0);
	});
