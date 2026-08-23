/**
 * The attached catalog as a new thread needs it: which project you were last
 * working in, and what to call it in the space of one word.
 *
 * Nothing here draws anything. The selector above it is only about presenting
 * a choice, and the order that choice is offered in is a fact about the
 * workspace rather than a fact about the menu.
 */

import type { Project, ThreadListItem } from "@artisan/protocol";
import { ThreadLastMessageAt } from "./thread-navigation";

/** One project as the selector needs it: an identity, a place in time, its weight. */
export type RecentProject = {
	/**
	 * When a message was last sent here. A project's own `updated_at` is a catalog
	 * row's stamp — it moves when the folder is attached or renamed — so the
	 * threads inside it are the better witness. The row is only the fallback for
	 * a project that has no thread messages yet.
	 */
	readonly last_message_at: string;
	readonly project: Project;
	readonly thread_count: number;
};

/**
 * Freshest first, which is the only order a project list means anything in:
 * the one you want next is nearly always the one you had last.
 */
export const RecentProjects = (
	projects: ReadonlyArray<Project>,
	threads: ReadonlyArray<ThreadListItem>,
): ReadonlyArray<RecentProject> => {
	const activity = new Map<string, { last_message_at: string; thread_count: number }>();
	for (const thread of threads) {
		const project_id = thread.primary_project?.project_id;
		if (project_id === undefined) continue;
		const last_message_at = ThreadLastMessageAt(thread);
		const seen = activity.get(project_id);
		if (seen === undefined) {
			activity.set(project_id, {
				last_message_at,
				thread_count: 1,
			});
			continue;
		}
		seen.thread_count += 1;
		if (last_message_at.localeCompare(seen.last_message_at) > 0)
			seen.last_message_at = last_message_at;
	}

	return [...projects]
		.map((project): RecentProject => {
			const seen = activity.get(project.project_id);
			return {
				last_message_at: seen?.last_message_at ?? project.updated_at,
				project,
				thread_count: seen?.thread_count ?? 0,
			};
		})
		.sort((left, right) => right.last_message_at.localeCompare(left.last_message_at));
};

/**
 * The project a fresh surface should open on: the one the reader named, else
 * the one they were last in, else simply the most recent. Resolved here rather
 * than in the route so "which project" has one answer wherever it is asked.
 */
export const PreferredProject = (
	recents: ReadonlyArray<RecentProject>,
	preferred_project_id: string | undefined,
): Project | undefined =>
	(preferred_project_id === undefined
		? undefined
		: recents.find((recent) => recent.project.project_id === preferred_project_id)?.project) ??
	recents[0]?.project;

/** Two letters is the most a small square can hold and still be read. */
export const ProjectMonogram = (name: string): string => {
	const parts = name.split(/[-_. /\\]/u).filter((part) => part.length > 0);
	if (parts.length === 0) return "··";
	return parts.length === 1
		? (parts[0] ?? name).slice(0, 2).toUpperCase()
		: `${parts[0]?.[0] ?? ""}${parts[1]?.[0] ?? ""}`.toUpperCase();
};
