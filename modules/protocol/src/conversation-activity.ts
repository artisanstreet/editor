import type { ConversationActivitySubagent, ConversationLifecycle } from "./conversation";

export interface ConversationActivityPresentationInput {
	readonly kind: string;
	readonly label: string;
	readonly status: ConversationLifecycle;
	readonly subagent?: ConversationActivitySubagent;
}

export interface ConversationActivityPresentation {
	readonly label: string;
}

/** The subset of an activity row needed to describe one grouped trace clause. */
export interface ConversationActivityGroupMember {
	readonly kind: string;
	readonly subagent?: ConversationActivitySubagent;
}

export interface ConversationActivityGroupPresentation {
	readonly count: number;
	/** Lowercase clause; the renderer capitalizes only the first clause in a chain. */
	readonly label: string;
}

type ActivityState = "active" | "completed" | "failed";

/** `interrupted` joins `cancelled`: stopped without completing, and not still running. */
const ActivityState = (status: ConversationLifecycle): ActivityState =>
	status === "failed" || status === "cancelled" || status === "interrupted"
		? "failed"
		: status === "completed"
			? "completed"
			: "active";

/**
 * The semantic bucket one activity kind falls in.
 *
 * Named separately from the copy so a reader-facing phrase and a count of the
 * same work cannot drift: both are looked up from this one classification.
 * `other` means the kind carried no recognised semantics and the activity's own
 * label is the only honest thing to show.
 */
export type ConversationActivityCategory =
	| "app_inspect"
	| "command"
	| "database"
	| "diff"
	| "file_delete"
	| "file_edit"
	| "file_read"
	| "file_search"
	| "git_status"
	| "integration"
	| "other"
	| "subagent"
	| "test"
	| "tool"
	| "typecheck"
	| "web_search";

const activity_category_labels: Record<ConversationActivityCategory, string> = {
	app_inspect: "App",
	command: "Command",
	database: "Database",
	diff: "Changes",
	file_delete: "Files",
	file_edit: "Files",
	file_read: "Files",
	file_search: "Files",
	git_status: "Git",
	integration: "Integrations",
	other: "Tools",
	subagent: "Subagents",
	test: "Tests",
	tool: "Tools",
	typecheck: "Types",
	web_search: "Web",
};

/** Stable foreground category copy shared by collapsed and expanded activity rows. */
export const GetConversationActivityCategoryLabel = (
	category: ConversationActivityCategory,
): string => activity_category_labels[category];

/** Provider kinds arrive in several shapes; compare them in one normalized form. */
const normalized_kind = (kind: string) =>
	kind.toLowerCase().replaceAll("-", ".").replaceAll("_", ".");

/**
 * Classifies one activity kind. Order is meaningful: the narrower semantics are
 * tested before the broad `tool` catch, so a kind naming both still reads as the
 * specific work it did.
 */
export const GetConversationActivityCategory = (kind: string): ConversationActivityCategory => {
	const value = normalized_kind(kind);

	if (value.includes("terminal") || value.includes("command") || value.includes("shell"))
		return "command";
	if (
		value === "file" ||
		value.includes("file.read") ||
		value.includes("workspace.read") ||
		value.endsWith(".read")
	)
		return "file_read";
	if (value.includes("file.delete")) return "file_delete";
	if (
		value.includes("file.edit") ||
		value.includes("file.write") ||
		value.includes("workspace.edit") ||
		value.includes("workspace.write")
	)
		return "file_edit";
	if (value.includes("workspace.search") || value.includes("file.list")) return "file_search";
	if (value === "search" || value.includes("web.search")) return "web_search";
	if (value.includes("test")) return "test";
	if (value.includes("typescript") || value.includes("typecheck")) return "typecheck";
	if (value.includes("git.status")) return "git_status";
	if (value.includes("diff")) return "diff";
	if (value.includes("database")) return "database";
	if (
		value.includes("preview") ||
		value.includes("browser") ||
		value.includes("ui.inspect") ||
		value.includes("accessibility")
	)
		return "app_inspect";
	if (value.includes("subagent") || value.includes("agent.activity")) return "subagent";
	if (value.includes("mcp") || value.includes("integration")) return "integration";
	if (value === "tool" || value.includes("tool") || value.includes("plugin")) return "tool";

	return "other";
};

/**
 * How one category talks about itself.
 *
 * `counted` is the lowercase clause a grouped chain contributes — "ran 4
 * commands" — so a caller can join several and capitalise only the first.
 */
interface ActivityCopy {
	readonly active: string;
	readonly completed: string;
	readonly counted: (count: number) => string;
	readonly failed: string;
}

const plural = (count: number, singular: string, many: string) =>
	count === 1 ? singular : `${count} ${many}`;

const activity_copy: Record<ConversationActivityCategory, ActivityCopy> = {
	app_inspect: {
		active: "Inspecting the app",
		completed: "Inspected the app",
		counted: (count) => (count === 1 ? "inspected the app" : `ran ${count} app inspections`),
		failed: "App inspection failed",
	},
	command: {
		active: "Running a command",
		completed: "Ran a command",
		counted: (count) => `ran ${plural(count, "a command", "commands")}`,
		failed: "Command failed",
	},
	database: {
		active: "Inspecting the database",
		completed: "Inspected the database",
		counted: (count) =>
			count === 1 ? "inspected the database" : `ran ${count} database inspections`,
		failed: "Database inspection failed",
	},
	diff: {
		active: "Reviewing changes",
		completed: "Reviewed changes",
		counted: (count) => (count === 1 ? "reviewed changes" : `reviewed ${count} diffs`),
		failed: "Change review failed",
	},
	file_delete: {
		active: "Deleting a file",
		completed: "Deleted a file",
		counted: (count) => `deleted ${plural(count, "a file", "files")}`,
		failed: "File delete failed",
	},
	file_edit: {
		active: "Editing a file",
		completed: "Edited a file",
		counted: (count) => `edited ${plural(count, "a file", "files")}`,
		failed: "File edit failed",
	},
	file_read: {
		active: "Reading a file",
		completed: "Read a file",
		counted: (count) => `read ${plural(count, "a file", "files")}`,
		failed: "File read failed",
	},
	file_search: {
		active: "Searching files",
		completed: "Searched files",
		counted: (count) => (count === 1 ? "searched files" : `searched ${count} files`),
		failed: "File search failed",
	},
	git_status: {
		active: "Checking Git status",
		completed: "Checked Git status",
		counted: (count) => (count === 1 ? "checked Git status" : `ran ${count} Git status checks`),
		failed: "Git status failed",
	},
	integration: {
		active: "Using an integration",
		completed: "Used an integration",
		counted: (count) => `used ${plural(count, "an integration", "integrations")}`,
		failed: "Integration failed",
	},
	other: {
		active: "Working",
		completed: "Worked",
		counted: (count) => `used ${plural(count, "a tool", "tools")}`,
		failed: "Work failed",
	},
	subagent: {
		active: "Talking to a subagent",
		completed: "Talked to a subagent",
		counted: (count) => `talked to ${plural(count, "a subagent", "subagents")}`,
		failed: "Subagent work failed",
	},
	test: {
		active: "Running tests",
		completed: "Ran tests",
		counted: (count) => (count === 1 ? "ran tests" : `ran ${count} test runs`),
		failed: "Tests failed",
	},
	tool: {
		active: "Using a tool",
		completed: "Used a tool",
		counted: (count) => `used ${plural(count, "a tool", "tools")}`,
		failed: "Tool failed",
	},
	typecheck: {
		active: "Checking types",
		completed: "Checked types",
		counted: (count) => (count === 1 ? "checked types" : `ran ${count} type checks`),
		failed: "Type check failed",
	},
	web_search: {
		active: "Searching the web",
		completed: "Searched the web",
		counted: (count) => (count === 1 ? "searched the web" : `ran ${count} web searches`),
		failed: "Web search failed",
	},
};

/** The clause one category contributes to a grouped chain's summary. */
export const GetConversationActivityCountLabel = (
	category: ConversationActivityCategory,
	count: number,
): string => activity_copy[category].counted(count);

/**
 * Describes one category inside an adjacent activity chain.
 *
 * Subagents are counted by durable Artisan identity rather than lifecycle rows.
 * That keeps discovered → running → waiting → complete updates as one person,
 * and lets the singular clause use the user-facing name without exposing a
 * provider thread id.
 */
export const GetConversationActivityGroupPresentation = (
	category: ConversationActivityCategory,
	activities: ReadonlyArray<ConversationActivityGroupMember>,
): ConversationActivityGroupPresentation => {
	if (category !== "subagent") {
		return {
			count: activities.length,
			label: GetConversationActivityCountLabel(category, activities.length),
		};
	}

	const named_agents = new Map<string, string>();
	let anonymous_count = 0;
	for (const activity of activities) {
		if (activity.subagent === undefined) {
			anonymous_count += 1;
			continue;
		}
		named_agents.set(activity.subagent.agent_id, activity.subagent.display_name);
	}
	const count = named_agents.size + anonymous_count;
	const only_name =
		count === 1 && anonymous_count === 0 ? named_agents.values().next().value : undefined;

	return {
		count,
		label:
			only_name === undefined
				? GetConversationActivityCountLabel(category, count)
				: `talked to ${only_name}`,
	};
};

/**
 * Maps provider-neutral activity semantics to stable human copy.
 *
 * Engine adapters retain provider names and raw provenance separately. This mapper deliberately
 * keys from normalized semantics so every renderer describes equivalent work the same way.
 */
export const GetConversationActivityPresentation = (
	activity: ConversationActivityPresentationInput,
): ConversationActivityPresentation => {
	const category = GetConversationActivityCategory(activity.kind);
	if (category === "other") return { label: activity.label };
	if (category === "subagent" && activity.subagent !== undefined) {
		const name = activity.subagent.display_name;
		return {
			label:
				activity.status === "failed" || activity.status === "cancelled"
					? `${name}'s work failed`
					: activity.status === "completed"
						? `Talked to ${name}`
						: `Talking to ${name}`,
		};
	}

	return { label: activity_copy[category][ActivityState(activity.status)] };
};
