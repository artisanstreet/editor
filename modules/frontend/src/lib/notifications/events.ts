import type { EventEnvelope } from "@artisan/protocol";

import type { SystemNotification } from "./contract";

/** What the renderer knows about itself when a durable event lands. */
export interface SystemNotificationContext {
	/** The thread whose transcript is on screen, if any. */
	readonly active_thread_id: string | undefined;
	/** Whether the reader is actually looking at this window. */
	readonly focused: boolean;
	/** Where the event's thread lives, resolved from the authoritative list. */
	readonly route_path: string;
	readonly thread_title: string | undefined;
}

export type SystemNotificationDecision =
	| { readonly _tag: "ignore" }
	| { readonly _tag: "revoke"; readonly id: string }
	| { readonly _tag: "show"; readonly notification: SystemNotification };

const ignore: SystemNotificationDecision = { _tag: "ignore" };

/**
 * The event kinds worth waking the host for. Filtering the stream before the
 * decision runs keeps heartbeat and filesystem traffic — which arrives
 * continuously — from costing a focus read apiece.
 */
export const IsNotifiableEvent = (envelope: EventEnvelope): boolean =>
	envelope.payload.type === "run.lifecycle" ||
	envelope.payload.type === "interaction.approval" ||
	envelope.payload.type === "interaction.question";

/** A notification body is one line of a toast, never a transcript. */
const summarize = (text: string): string => {
	const collapsed = text.replaceAll(/\s+/gu, " ").trim();
	return collapsed.length <= 120 ? collapsed : `${collapsed.slice(0, 119).trimEnd()}…`;
};

/**
 * The identity a pending interaction's toast is posted under. Resolving the
 * interaction anywhere — in the app, from another client, by cancelling the
 * run — revokes it by the same name.
 */
const InteractionNotificationId = (envelope: EventEnvelope): string | undefined => {
	const payload = envelope.payload;
	if (payload.type === "interaction.approval") return `approval:${payload.approval_id}`;
	if (payload.type === "interaction.question") return `question:${payload.question_id}`;
	return undefined;
};

const NotificationFor = (
	envelope: EventEnvelope,
	context: SystemNotificationContext,
): SystemNotification | undefined => {
	const payload = envelope.payload;
	const shared = {
		route_path: context.route_path,
		title: context.thread_title ?? "Artisan",
	};

	if (payload.type === "run.lifecycle") {
		/**
		 * Only the two outcomes the reader did not already cause. A cancelled or
		 * interrupted run is the reader's own action arriving back at them, and
		 * the intermediate states are what the transcript is for.
		 */
		const run_identity = envelope.run_id ?? envelope.thread_id;
		if (payload.state === "completed")
			return {
				...shared,
				body: "Finished.",
				category: "run_completed",
				id: `run:${run_identity}`,
			};
		if (payload.state === "failed")
			return {
				...shared,
				body: "The run failed.",
				category: "run_failed",
				id: `run:${run_identity}`,
			};
		return undefined;
	}

	const id = InteractionNotificationId(envelope);
	if (id === undefined) return undefined;

	if (payload.type === "interaction.approval" && payload.state === "requested")
		return {
			...shared,
			body: `Needs approval — ${summarize(payload.description)}`,
			category: "approval",
			id,
		};

	if (payload.type === "interaction.question" && payload.state === "requested")
		return {
			...shared,
			body: `Has a question — ${summarize(payload.text)}`,
			category: "question",
			id,
		};

	return undefined;
};

/**
 * Decides what one durable event should do to the host's notification centre.
 *
 * Pure by design: the whole policy — which events matter, what they say, when
 * they are redundant, and what revokes them — is decided here and asserted in
 * tests, leaving the service to do nothing but carry the answer out.
 */
export const SystemNotificationDecisionFor = (
	envelope: EventEnvelope,
	context: SystemNotificationContext,
): SystemNotificationDecision => {
	const payload = envelope.payload;
	const resolved =
		(payload.type === "interaction.approval" || payload.type === "interaction.question") &&
		payload.state === "resolved";
	if (resolved) {
		const id = InteractionNotificationId(envelope);
		return id === undefined ? ignore : { _tag: "revoke", id };
	}

	const notification = NotificationFor(envelope, context);
	if (notification === undefined) return ignore;

	/**
	 * A thread open in a focused window has already said everything the toast
	 * would, in more detail and in place. Anything else — a second thread,
	 * another window, another application entirely — has not.
	 */
	if (context.focused && context.active_thread_id === envelope.thread_id) return ignore;

	return { _tag: "show", notification };
};
