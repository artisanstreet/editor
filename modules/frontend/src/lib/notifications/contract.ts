import { Context, Effect } from "effect";

/**
 * What the host will let the page post right now.
 *
 * `unsupported` is a real answer rather than a failure: the desktop shell and
 * every current browser expose the API, but a hardened or embedded webview may
 * not, and the settings row has to say so instead of offering a switch that can
 * never take.
 */
export type SystemNotificationPermission = "unsupported" | "default" | "granted" | "denied";

export type SystemNotificationCategory = "approval" | "question" | "run_completed" | "run_failed";

/** One notification the renderer is asking its host to post. */
export interface SystemNotification {
	readonly body: string;
	readonly category: SystemNotificationCategory;
	/**
	 * Derived from the durable identity behind the notification — an approval,
	 * a question, a run — so the same fact never posts twice and resolving it
	 * inside the app can revoke the toast by name.
	 */
	readonly id: string;
	/** Where clicking the notification takes the reader. */
	readonly route_path: string;
	readonly title: string;
}

/**
 * The host's notification centre, as the one capability the renderer needs from
 * it. Both surfaces are served by the same web adapter: the desktop shell has
 * no preload and no IPC, so its renderer reaches the OS exactly the way a
 * paired browser tab does, and only the permission answer differs.
 */
export class SystemNotificationPresenter extends Context.Service<
	SystemNotificationPresenter,
	{
		readonly Dismiss: (id: string) => Effect.Effect<void>;
		/**
		 * Read on demand rather than observed. Permission changes in host
		 * settings, outside anything the page can see, so the settings surface
		 * re-reads it on arrival and behind its own explicit control instead of
		 * opening a listener that would have to be audited as another callback
		 * ingress for a state nobody changes twice.
		 */
		readonly Permission: Effect.Effect<SystemNotificationPermission>;
		readonly Request: Effect.Effect<SystemNotificationPermission>;
		readonly Show: (
			notification: SystemNotification,
			on_activate: (notification: SystemNotification) => void,
		) => Effect.Effect<void>;
	}
>()("Artisan/SystemNotificationPresenter") {}
