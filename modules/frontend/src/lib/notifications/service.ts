import type { EventEnvelope } from "@artisan/protocol";
import { Context, Effect, Layer, Queue, Stream, SubscriptionRef } from "effect";

import { RouteNavigation } from "../browser/route-navigation";
import { RunBrowserDom } from "../browser/dom";
import { SystemNotificationPresenter, type SystemNotificationPermission } from "./contract";
import { SystemNotificationDecisionFor, type SystemNotificationContext } from "./events";
import { NotificationPreferences } from "./preferences";

/**
 * How long a posted notification stays up when nobody answers it.
 *
 * A toast that waits forever is a toast that covers a game, a call, or a full
 * screen of someone else's work until it is manually cleared. Artisan's
 * notifications are a nudge, never the place a decision is made: the approval
 * or question stays pending in the thread for as long as it takes, so dropping
 * the toast costs the reader nothing.
 */
export const AutoDismissDelay = "5 seconds";

/** Everything the settings surface needs to describe the feature's state. */
export interface SystemNotificationSettings {
	/** What the reader asked for, which survives a permission they cannot grant. */
	readonly enabled: boolean;
	readonly permission: SystemNotificationPermission;
}

/**
 * What stands between what the reader asked for and a notification arriving.
 *
 * The two failing answers are worth separating because the way out of them is
 * different: a refusal has to be undone in host settings, where no page can
 * reach, while a dismissed prompt can simply be asked again.
 */
export type SystemNotificationGap = "none" | "blocked" | "unprompted" | "unsupported";

export const SystemNotificationGapFor = (
	settings: SystemNotificationSettings,
): SystemNotificationGap => {
	if (settings.permission === "unsupported") return "unsupported";
	if (!settings.enabled || settings.permission === "granted") return "none";
	return settings.permission === "denied" ? "blocked" : "unprompted";
};

/** Notifications are on and the host will actually post them. */
export const SystemNotificationsAreActive = (settings: SystemNotificationSettings): boolean =>
	settings.enabled && settings.permission === "granted";

export class SystemNotifications extends Context.Service<
	SystemNotifications,
	{
		readonly Changes: Stream.Stream<SystemNotificationSettings>;
		readonly Current: Effect.Effect<SystemNotificationSettings>;
		readonly Disable: Effect.Effect<SystemNotificationSettings>;
		/** Stores the intent and asks the host for permission in one gesture. */
		readonly Enable: Effect.Effect<SystemNotificationSettings>;
		readonly Handle: (
			envelope: EventEnvelope,
			context: SystemNotificationContext,
		) => Effect.Effect<void>;
		/** Re-reads host permission, for a reader who just changed it elsewhere. */
		readonly Refresh: Effect.Effect<SystemNotificationSettings>;
	}
>()("Artisan/SystemNotifications") {}

export const SystemNotificationsLive = Layer.effect(
	SystemNotifications,
	Effect.gen(function* () {
		const presenter = yield* SystemNotificationPresenter;
		const preferences = yield* NotificationPreferences;
		const navigation = yield* RouteNavigation;
		const service_scope = yield* Effect.scope;

		const stored = yield* preferences.Load;
		const state = yield* SubscriptionRef.make<SystemNotificationSettings>({
			enabled: stored.enabled,
			permission: yield* presenter.Permission,
		});

		/**
		 * Notification clicks are a foreign ingress: the host invokes them on its
		 * own callback, outside any fiber. This queue is where they re-enter the
		 * runtime, exactly as banner actions do.
		 */
		const activations = yield* Queue.unbounded<{ readonly route_path: string }>();
		const ConsumeActivations = Effect.gen(function* () {
			while (true) {
				const activation = yield* Queue.take(activations);
				/**
				 * Raises the tab, or the desktop window, before the route changes.
				 * The sandboxed shell has no IPC to the main process, so this is a
				 * page-level request the host may decline — the navigation still
				 * lands, and the reader finds the thread open when they return.
				 */
				yield* RunBrowserDom(() =>
					(globalThis as { readonly focus?: () => void }).focus?.(),
				).pipe(Effect.ignore);
				yield* navigation.Navigate(activation.route_path).pipe(Effect.ignore);
			}
		});
		yield* Effect.forkIn(ConsumeActivations, service_scope);

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});

		const Refresh = Effect.gen(function* () {
			const permission = yield* presenter.Permission;
			yield* SubscriptionRef.update(state, (current) => ({ ...current, permission }));
			return yield* Current;
		});

		const SetEnabled = (enabled: boolean) =>
			Effect.gen(function* () {
				yield* preferences.Save({ version: 1, enabled });
				yield* SubscriptionRef.update(state, (current) => ({ ...current, enabled }));
			});

		/**
		 * The intent is stored even when the host refuses, so unblocking Artisan
		 * later needs no second trip through settings.
		 */
		const Enable = Effect.gen(function* () {
			const permission = yield* presenter.Request;
			yield* SetEnabled(true);
			yield* SubscriptionRef.update(state, (current) => ({ ...current, permission }));
			return yield* Current;
		});

		const Disable = Effect.gen(function* () {
			yield* SetEnabled(false);
			return yield* Current;
		});

		const Handle = (envelope: EventEnvelope, context: SystemNotificationContext) =>
			Effect.gen(function* () {
				const decision = SystemNotificationDecisionFor(envelope, context);
				if (decision._tag === "ignore") return;

				/**
				 * Revocation ignores both the switch and the permission: a toast
				 * already on screen for an approval that has just been answered
				 * elsewhere must come down even if notifications were turned off
				 * in the meantime.
				 */
				if (decision._tag === "revoke") {
					yield* presenter.Dismiss(decision.id);
					return;
				}

				const settings = yield* Current;
				if (!SystemNotificationsAreActive(settings)) return;

				yield* presenter.Show(decision.notification, (notification) => {
					Queue.offerUnsafe(activations, { route_path: notification.route_path });
				});
				yield* Effect.forkIn(
					Effect.gen(function* () {
						yield* Effect.sleep(AutoDismissDelay);
						yield* presenter.Dismiss(decision.notification.id);
					}),
					service_scope,
				);
			});

		return SystemNotifications.of({
			Changes: SubscriptionRef.changes(state),
			Current,
			Disable,
			Enable,
			Handle,
			Refresh,
		});
	}),
);
