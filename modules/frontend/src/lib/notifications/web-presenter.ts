import { Effect, Layer } from "effect";

import { RunBrowserDom } from "../browser/dom";
import {
	SystemNotificationPresenter,
	type SystemNotification,
	type SystemNotificationPermission,
} from "./contract";

/**
 * The host notification surface, described structurally rather than pulled from
 * the DOM lib. The renderer is typechecked alongside Node-side workspace code,
 * and a narrow shape is also the honest one: these few members are the entire
 * dependency.
 */
interface HostNotification {
	close(): void;
	onclick: ((event: unknown) => void) | null;
	onclose: ((event: unknown) => void) | null;
}

interface HostNotificationConstructor {
	new (
		title: string,
		options: { readonly body: string; readonly icon: string; readonly tag: string },
	): HostNotification;
	readonly permission: string;
	requestPermission(): Promise<string>;
}

const NotificationApi = (): HostNotificationConstructor | undefined =>
	(globalThis as { readonly Notification?: HostNotificationConstructor }).Notification;

const NormalizePermission = (value: string): SystemNotificationPermission =>
	value === "granted" || value === "denied" ? value : "default";

const ReadPermission = (): SystemNotificationPermission => {
	const api = NotificationApi();
	return api === undefined ? "unsupported" : NormalizePermission(api.permission);
};

/** Served from the frontend root under both the app scheme and a paired origin. */
const notification_icon = "/barekey-logo.png";

/**
 * The one adapter both surfaces use.
 *
 * The desktop shell deliberately ships no preload and no IPC, so its renderer
 * is a sandboxed Chromium page that reaches the operating system's notification
 * centre through the very same web API a paired browser tab uses. That is why
 * there is no second, Electron-specific presenter: the surfaces differ in what
 * permission they start with, not in how a notification is posted.
 */
export const WebSystemNotificationPresenterLive = Layer.effect(
	SystemNotificationPresenter,
	Effect.gen(function* () {
		/**
		 * Posted notifications still on screen, by the durable identity they were
		 * posted under. Every entry leaves by dismissal, by the reader closing it,
		 * or by the auto-dismiss the service always schedules, so the map cannot
		 * outgrow what is actually visible.
		 */
		const live = new Map<string, HostNotification>();

		const Dismiss = (id: string) =>
			Effect.gen(function* () {
				const posted = live.get(id);
				if (posted === undefined) return;
				live.delete(id);
				yield* RunBrowserDom(() => {
					posted.close();
				}).pipe(Effect.ignore);
			});

		const Permission = Effect.gen(function* () {
			return yield* RunBrowserDom(ReadPermission).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return "unsupported" as const;
					}),
				),
			);
		});

		const Request = Effect.gen(function* () {
			const api = NotificationApi();
			if (api === undefined) return "unsupported" as const;
			/**
			 * A host that has already refused resolves this immediately with
			 * "denied" and shows nothing — no browser lets a page prompt its way
			 * past a block. Asking anyway is still right: it is the only way to
			 * learn the current answer in one call, and the settings surface is
			 * what explains a refusal when it comes back.
			 */
			const answered = yield* Effect.tryPromise({
				try: () => api.requestPermission(),
				catch: (cause) => cause,
			}).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return undefined;
					}),
				),
			);

			return answered === undefined ? ReadPermission() : NormalizePermission(answered);
		});

		const Show = (
			notification: SystemNotification,
			on_activate: (notification: SystemNotification) => void,
		) =>
			Effect.gen(function* () {
				const api = NotificationApi();
				if (api === undefined || NormalizePermission(api.permission) !== "granted") return;
				/** Replacing our own record first keeps one identity to one toast. */
				yield* Dismiss(notification.id);

				const posted = yield* RunBrowserDom(
					() =>
						new api(notification.title, {
							body: notification.body,
							icon: notification_icon,
							/** The host coalesces same-tag toasts exactly as we do. */
							tag: notification.id,
						}),
				).pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							return undefined;
						}),
					),
				);
				if (posted === undefined) return;

				live.set(notification.id, posted);
				posted.onclick = () => {
					live.delete(notification.id);
					posted.close();
					on_activate(notification);
				};
				posted.onclose = () => {
					live.delete(notification.id);
				};
			});

		return SystemNotificationPresenter.of({ Dismiss, Permission, Request, Show });
	}),
);
