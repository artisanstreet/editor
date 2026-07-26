import type { ConversationLifecycle } from "./conversation";

export interface ConversationActivityPresentationInput {
	readonly kind: string;
	readonly label: string;
	readonly status: ConversationLifecycle;
}

export interface ConversationActivityPresentation {
	readonly label: string;
}

type ActivityState = "active" | "completed" | "failed";

const ActivityState = (status: ConversationLifecycle): ActivityState =>
	status === "failed" || status === "cancelled"
		? "failed"
		: status === "completed"
			? "completed"
			: "active";

const StateLabel = (
	state: ActivityState,
	labels: { readonly active: string; readonly completed: string; readonly failed: string },
) => labels[state];

/**
 * Maps provider-neutral activity semantics to stable human copy.
 *
 * Engine adapters retain provider names and raw provenance separately. This mapper deliberately
 * keys from normalized semantics so every renderer describes equivalent work the same way.
 */
export const GetConversationActivityPresentation = (
	activity: ConversationActivityPresentationInput,
): ConversationActivityPresentation => {
	const kind = activity.kind.toLowerCase().replaceAll("-", ".").replaceAll("_", ".");
	const state = ActivityState(activity.status);

	if (kind.includes("terminal") || kind.includes("command") || kind.includes("shell")) {
		return {
			label: StateLabel(state, {
				active: "Running a command",
				completed: "Ran a command",
				failed: "Command failed",
			}),
		};
	}
	if (
		kind === "file" ||
		kind.includes("file.read") ||
		kind.includes("workspace.read") ||
		kind.endsWith(".read")
	) {
		return {
			label: StateLabel(state, {
				active: "Reading a file",
				completed: "Read a file",
				failed: "File read failed",
			}),
		};
	}
	if (
		kind.includes("file.write") ||
		kind.includes("workspace.edit") ||
		kind.includes("workspace.write")
	) {
		return {
			label: StateLabel(state, {
				active: "Editing a file",
				completed: "Edited a file",
				failed: "File edit failed",
			}),
		};
	}
	if (kind.includes("workspace.search") || kind.includes("file.list")) {
		return {
			label: StateLabel(state, {
				active: "Searching files",
				completed: "Searched files",
				failed: "File search failed",
			}),
		};
	}
	if (kind === "search" || kind.includes("web.search")) {
		return {
			label: StateLabel(state, {
				active: "Searching the web",
				completed: "Searched the web",
				failed: "Web search failed",
			}),
		};
	}
	if (kind.includes("test")) {
		return {
			label: StateLabel(state, {
				active: "Running tests",
				completed: "Ran tests",
				failed: "Tests failed",
			}),
		};
	}
	if (kind.includes("typescript") || kind.includes("typecheck")) {
		return {
			label: StateLabel(state, {
				active: "Checking types",
				completed: "Checked types",
				failed: "Type check failed",
			}),
		};
	}
	if (kind.includes("git.status")) {
		return {
			label: StateLabel(state, {
				active: "Checking Git status",
				completed: "Checked Git status",
				failed: "Git status failed",
			}),
		};
	}
	if (kind.includes("diff")) {
		return {
			label: StateLabel(state, {
				active: "Reviewing changes",
				completed: "Reviewed changes",
				failed: "Change review failed",
			}),
		};
	}
	if (kind.includes("database")) {
		return {
			label: StateLabel(state, {
				active: "Inspecting the database",
				completed: "Inspected the database",
				failed: "Database inspection failed",
			}),
		};
	}
	if (
		kind.includes("preview") ||
		kind.includes("browser") ||
		kind.includes("ui.inspect") ||
		kind.includes("accessibility")
	) {
		return {
			label: StateLabel(state, {
				active: "Inspecting the app",
				completed: "Inspected the app",
				failed: "App inspection failed",
			}),
		};
	}
	if (kind.includes("subagent") || kind.includes("agent.activity")) {
		return {
			label: StateLabel(state, {
				active: "Working with a subagent",
				completed: "Worked with a subagent",
				failed: "Subagent work failed",
			}),
		};
	}
	if (kind.includes("mcp") || kind.includes("integration")) {
		return {
			label: StateLabel(state, {
				active: "Using an integration",
				completed: "Used an integration",
				failed: "Integration failed",
			}),
		};
	}
	if (kind === "tool" || kind.includes("tool") || kind.includes("plugin")) {
		return {
			label: StateLabel(state, {
				active: "Using a tool",
				completed: "Used a tool",
				failed: "Tool failed",
			}),
		};
	}

	return { label: activity.label };
};
