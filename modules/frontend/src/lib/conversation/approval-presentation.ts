import type { ConversationItem } from "@artisan/protocol";

type ApprovalItem = Extract<ConversationItem, { type: "approval" }>;

export interface ApprovalPresentation {
	readonly approve_label: string;
	readonly command?: string;
	readonly cwd?: string;
	readonly description?: string;
	readonly kind: "action" | "command" | "file_change";
	readonly title: string;
}

const protocol_plumbing = /^item\/.+\/requestApproval(?:\s+for\s+\S+)?$/u;

const LegacyDescription = (item: ApprovalItem) => {
	const prompt = item.prompt.trim();

	return prompt.length === 0 || protocol_plumbing.test(prompt) ? undefined : prompt;
};

const StateTitle = (item: ApprovalItem, noun: string) => {
	if (item.state === "requested") return undefined;
	if (item.state === "approved") return `${noun} approved`;
	if (item.state === "rejected") return `${noun} denied`;
	return `${noun} cancelled`;
};

export const GetApprovalPresentation = (item: ApprovalItem): ApprovalPresentation => {
	const request = item.request;
	const reason = request?.reason ?? LegacyDescription(item);

	if (request?.kind === "command") {
		return {
			approve_label: "Run command",
			...(request.command === undefined ? {} : { command: request.command }),
			...(request.cwd === undefined ? {} : { cwd: request.cwd }),
			...(item.state === "requested" && reason !== undefined ? { description: reason } : {}),
			kind: "command",
			title: StateTitle(item, "Command") ?? "Run this command?",
		};
	}

	if (request?.kind === "file_change") {
		return {
			approve_label: "Apply changes",
			...(item.state === "requested" && reason !== undefined ? { description: reason } : {}),
			kind: "file_change",
			title: StateTitle(item, "Changes") ?? "Apply these changes?",
		};
	}

	return {
		approve_label: "Approve",
		...(item.state === "requested"
			? {
					description: reason ?? "Artisan needs your approval before it can continue.",
				}
			: {}),
		kind: "action",
		title: StateTitle(item, "Action") ?? "Allow this action?",
	};
};
