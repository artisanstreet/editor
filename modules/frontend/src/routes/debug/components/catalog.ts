export interface ComponentGalleryEntry {
	readonly description: string;
	readonly group: string;
	readonly id: string;
	readonly label: string;
}

/**
 * The gallery is intentionally curated rather than discovered from the file
 * tree. A component earns a place when it represents a distinct thread state
 * somebody needs to inspect, and the order reads like a thread from send to
 * settlement and recovery.
 */
export const component_gallery_entries = [
	{
		description: "A dense, realistic conversation inside the production thread workspace.",
		group: "Thread",
		id: "full-thread",
		label: "Full thread",
	},
	{
		description: "The authored prompt card aligned to the conversation’s right edge.",
		group: "Messages",
		id: "user-message",
		label: "User message",
	},
	{
		description:
			"A user prompt with resolved image thumbnails and the real image viewer interaction.",
		group: "Messages",
		id: "image-message",
		label: "Message with images",
	},
	{
		description: "A settled assistant response using the production Markdown renderer.",
		group: "Messages",
		id: "assistant-message",
		label: "Assistant response",
	},
	{
		description: "An assistant response while tokens are still arriving.",
		group: "Messages",
		id: "streaming-message",
		label: "Streaming response",
	},
	{
		description: "Provider-visible reasoning with the active shimmer treatment.",
		group: "Messages",
		id: "reasoning-summary",
		label: "Reasoning summary",
	},
	{
		description:
			"A live turn waiting on its provider, including elapsed-time and disclosure behavior.",
		group: "Work",
		id: "active-work",
		label: "Active work session",
	},
	{
		description:
			"A live turn's newest thinking paragraph, whole, replaced as the next one opens.",
		group: "Work",
		id: "thinking-summary",
		label: "Thinking summary",
	},
	{
		description: "A naturally completed turn rendered as settled history.",
		group: "Work",
		id: "completed-work",
		label: "Completed work session",
	},
	{
		description:
			"A failed provider attempt with its diagnostic trace available for inspection.",
		group: "Work",
		id: "failed-work",
		label: "Failed work session",
	},
	{
		description: "One provider activity row before it is grouped into a longer trace.",
		group: "Work",
		id: "activity-row",
		label: "Activity row",
	},
	{
		description:
			"A mixed tool chain with commands, search, reasoning, and an active operation.",
		group: "Work",
		id: "activity-trace",
		label: "Activity trace",
	},
	{
		description: "The aggregate changed-files card with paths and diff counts.",
		group: "Work",
		id: "edited-files",
		label: "Edited files",
	},
	{
		description: "A command permission request with its exact command and working directory.",
		group: "Requests",
		id: "command-approval",
		label: "Command approval",
	},
	{
		description: "A provider question waiting for a short user answer.",
		group: "Requests",
		id: "question",
		label: "Question",
	},
	{
		description:
			"A model-specific usage interruption with reset countdown and verified alternative.",
		group: "Recovery",
		id: "usage-limit",
		label: "Usage limit",
	},
	{
		description: "The compact historical state after a usage interruption has continued.",
		group: "Recovery",
		id: "usage-continued",
		label: "Usage continued",
	},
	{
		description:
			"A catalog-backed provider failure with code, explanation, and reset evidence.",
		group: "Recovery",
		id: "provider-error",
		label: "Provider error",
	},
	{
		description: "The chapter divider while context compaction is in progress.",
		group: "Boundaries",
		id: "compacting",
		label: "Compacting",
	},
	{
		description: "The same chapter divider after compaction has settled.",
		group: "Boundaries",
		id: "compacted",
		label: "Compacted",
	},
	{
		description: "A native continuation handing the thread from one model to another.",
		group: "Boundaries",
		id: "model-handoff",
		label: "Model handoff",
	},
	{
		description: "The composer’s context-window control and its interactive usage detail.",
		group: "Controls",
		id: "context-window",
		label: "Context window",
	},
	{
		description: "The response hover footer with copy action and relative settlement time.",
		group: "Controls",
		id: "turn-actions",
		label: "Turn actions",
	},
] as const satisfies ReadonlyArray<ComponentGalleryEntry>;

export type ComponentGalleryId = (typeof component_gallery_entries)[number]["id"];

/** Unknown or absent deep links open the first specimen rather than a blank stage. */
export const component_gallery_index_for = (requested_id: string | null | undefined): number => {
	const index = component_gallery_entries.findIndex((entry) => entry.id === requested_id);
	return index < 0 ? 0 : index;
};

/** The arrows loop so every specimen stays one click away from the next. */
export const component_gallery_neighbor = (
	index: number,
	delta: -1 | 1,
): (typeof component_gallery_entries)[number] => {
	const total = component_gallery_entries.length;
	return (
		component_gallery_entries[(index + delta + total) % total] ?? component_gallery_entries[0]
	);
};
