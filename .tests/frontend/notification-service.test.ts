import type { EventEnvelope } from "@artisan/protocol";
import { Effect, Layer } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import { RouteNavigation } from "../../modules/frontend/src/lib/browser/route-navigation";
import {
	SystemNotificationPresenter,
	type SystemNotification,
	type SystemNotificationPermission,
} from "../../modules/frontend/src/lib/notifications/contract";
import type { SystemNotificationContext } from "../../modules/frontend/src/lib/notifications/events";
import {
	NotificationPreferences,
	type NotificationState,
} from "../../modules/frontend/src/lib/notifications/preferences";
import {
	SystemNotificationGapFor,
	SystemNotifications,
	SystemNotificationsLive,
} from "../../modules/frontend/src/lib/notifications/service";

interface Recorder {
	readonly activate: ((notification: SystemNotification) => void) | undefined;
	readonly dismissed: ReadonlyArray<string>;
	readonly navigated: ReadonlyArray<string>;
	readonly shown: ReadonlyArray<SystemNotification>;
	readonly stored: NotificationState;
}

const envelope = (payload: unknown): EventEnvelope =>
	({
		causation_id: "cause_1",
		correlation_id: "correlation_1",
		journal_sequence: 1,
		kind: "event",
		payload,
		run_id: "run_1",
		sequence: 1,
		stream_id: "stream_1",
		thread_id: "thread_1",
	}) as EventEnvelope;

const run_completed = envelope({
	state: "completed",
	type: "run.lifecycle",
	working_directory: "/w",
});

const approval_requested = envelope({
	approval_id: "approval_1",
	description: "Delete the stale worktree",
	state: "requested",
	type: "interaction.approval",
});

const approval_resolved = envelope({
	approval_id: "approval_1",
	approved: true,
	description: "Delete the stale worktree",
	state: "resolved",
	type: "interaction.approval",
});

const context: SystemNotificationContext = {
	active_thread_id: undefined,
	focused: false,
	route_path: "/t/project_1/thread_1",
	thread_title: "Land the notification service",
};

/** Lets the activation consumer drain what a host callback just offered. */
const settle = Effect.gen(function* () {
	for (let attempt = 0; attempt < 10; attempt += 1) yield* Effect.yieldNow;
});

/**
 * Drives the real service against recording doubles for its two foreign
 * boundaries — the host notification centre and routed navigation — on a test
 * clock, so the auto-dismiss delay is asserted rather than waited out.
 */
const Run = <Value>(
	options: {
		readonly permission?: SystemNotificationPermission;
		readonly stored?: NotificationState;
	},
	Body: (read: () => Recorder) => Effect.Effect<Value, never, SystemNotifications>,
): Promise<Value> => {
	const permission = options.permission ?? "granted";
	const shown: Array<SystemNotification> = [];
	const dismissed: Array<string> = [];
	const navigated: Array<string> = [];
	let stored: NotificationState = options.stored ?? { version: 1, enabled: false };
	let activate: ((notification: SystemNotification) => void) | undefined;

	const dependencies = Layer.mergeAll(
		Layer.succeed(
			SystemNotificationPresenter,
			SystemNotificationPresenter.of({
				Dismiss: (id) =>
					Effect.sync(() => {
						dismissed.push(id);
					}),
				Permission: Effect.succeed(permission),
				Request: Effect.succeed(permission),
				Show: (notification, on_activate) =>
					Effect.sync(() => {
						activate = on_activate;
						shown.push(notification);
					}),
			}),
		),
		Layer.succeed(
			NotificationPreferences,
			NotificationPreferences.of({
				Load: Effect.sync(() => stored),
				Save: (state) =>
					Effect.sync(() => {
						stored = state;
					}),
			}),
		),
		Layer.succeed(
			RouteNavigation,
			RouteNavigation.of({
				Navigate: (path) =>
					Effect.sync(() => {
						navigated.push(String(path));
					}),
			}),
		),
	);

	const read = (): Recorder => ({ activate, dismissed, navigated, shown, stored });

	return Effect.runPromise(
		Body(read).pipe(
			Effect.provide(SystemNotificationsLive.pipe(Layer.provide(dependencies))),
			Effect.provide(TestClock.layer()),
		),
	);
};

describe("SystemNotifications", () => {
	it("posts nothing until the reader has turned notifications on", async () => {
		const shown = await Run({ stored: { version: 1, enabled: false } }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				yield* notifications.Handle(run_completed, context);
				return read().shown;
			}),
		);

		expect(shown).toEqual([]);
	});

	it("posts nothing while the host refuses, even once the reader has asked", async () => {
		const shown = await Run(
			{ permission: "denied", stored: { version: 1, enabled: true } },
			(read) =>
				Effect.gen(function* () {
					const notifications = yield* SystemNotifications;
					yield* notifications.Handle(run_completed, context);
					return read().shown;
				}),
		);

		expect(shown).toEqual([]);
	});

	it("keeps the reader's intent when the host refuses, so unblocking needs no second visit", async () => {
		const result = await Run({ permission: "denied" }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				const settings = yield* notifications.Enable;
				return { settings, stored: read().stored };
			}),
		);

		expect(result.settings).toEqual({ enabled: true, permission: "denied" });
		expect(result.stored).toEqual({ version: 1, enabled: true });
	});

	it("clears an unanswered notification once the delay elapses, and not before", async () => {
		const result = await Run({ stored: { version: 1, enabled: true } }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				yield* notifications.Handle(run_completed, context);
				expect(read().shown).toHaveLength(1);

				yield* TestClock.adjust(4_000);
				const early = [...read().dismissed];
				yield* TestClock.adjust(1_000);

				return { early, late: read().dismissed };
			}),
		);

		expect(result.early).toEqual([]);
		expect(result.late).toEqual(["run:run_1"]);
	});

	it("revokes a pending approval the moment it is resolved elsewhere", async () => {
		const dismissed = await Run({ stored: { version: 1, enabled: true } }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				yield* notifications.Handle(approval_requested, context);
				yield* notifications.Handle(approval_resolved, context);
				return [...read().dismissed];
			}),
		);

		expect(dismissed).toEqual(["approval:approval_1"]);
	});

	it("revokes a resolved interaction even after notifications are turned off", async () => {
		const dismissed = await Run({ stored: { version: 1, enabled: false } }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				yield* notifications.Handle(approval_resolved, context);
				return [...read().dismissed];
			}),
		);

		expect(dismissed).toEqual(["approval:approval_1"]);
	});

	it("separates a refusal from a prompt nobody answered", () => {
		expect(SystemNotificationGapFor({ enabled: true, permission: "denied" })).toBe("blocked");
		expect(SystemNotificationGapFor({ enabled: true, permission: "default" })).toBe(
			"unprompted",
		);
		expect(SystemNotificationGapFor({ enabled: true, permission: "granted" })).toBe("none");
		/** Nothing is in the way of a feature the reader has not asked for. */
		expect(SystemNotificationGapFor({ enabled: false, permission: "denied" })).toBe("none");
		expect(SystemNotificationGapFor({ enabled: false, permission: "unsupported" })).toBe(
			"unsupported",
		);
	});

	it("opens the thread a clicked notification belongs to", async () => {
		const navigated = await Run({ stored: { version: 1, enabled: true } }, (read) =>
			Effect.gen(function* () {
				const notifications = yield* SystemNotifications;
				yield* notifications.Handle(approval_requested, context);
				read().activate?.(read().shown[0]!);
				yield* settle;
				return [...read().navigated];
			}),
		);

		expect(navigated).toEqual(["/t/project_1/thread_1"]);
	});
});
