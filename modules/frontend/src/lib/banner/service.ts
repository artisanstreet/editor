import { Context, Data, Effect, Layer, Queue } from "effect";

export type BannerSeverity = "error" | "warning" | "info" | "success";
export type BannerActionIcon = "player-play" | "refresh";

interface BannerActionBase {
	readonly icon?: BannerActionIcon;
	readonly id: string;
	readonly label: string;
}

export interface BannerEffectAction extends BannerActionBase {
	readonly Execute: Effect.Effect<void>;
}

export interface BannerLinkAction extends BannerActionBase {
	readonly Execute?: Effect.Effect<void>;
	readonly href: string;
}

export type BannerAction = BannerEffectAction | BannerLinkAction;
export type BannerExecutableAction =
	| BannerEffectAction
	| (BannerLinkAction & {
			readonly Execute: Effect.Effect<void>;
	  });

export const is_banner_executable_action = (
	action: BannerAction,
): action is BannerExecutableAction => action.Execute !== undefined;

export interface BannerOptions {
	readonly actions?: ReadonlyArray<BannerAction>;
	readonly code?: string;
	readonly description?: string;
	readonly duration_ms?: number;
	readonly id?: string;
	readonly metadata?: Readonly<Record<string, string>>;
}

export interface BannerEvent extends BannerOptions {
	readonly severity: BannerSeverity;
	readonly title: string;
}

export interface BannerReportEvent extends Omit<BannerEvent, "actions"> {
	readonly actions?: ReadonlyArray<Pick<BannerAction, "icon" | "id" | "label">>;
}

export class BannerReporterError extends Data.TaggedError("BannerReporterError")<{
	readonly cause: unknown;
}> {}

export class BannerPresenter extends Context.Service<
	BannerPresenter,
	{
		readonly Dismiss: (id: string) => Effect.Effect<void>;
		readonly Show: (
			event: BannerEvent,
			on_action: (action: BannerExecutableAction) => void,
		) => Effect.Effect<void>;
	}
>()("Artisan/BannerPresenter") {}

export class BannerReporter extends Context.Service<
	BannerReporter,
	{
		readonly Report: (event: BannerReportEvent) => Effect.Effect<void, BannerReporterError>;
	}
>()("Artisan/BannerReporter") {}

export class BannerService extends Context.Service<
	BannerService,
	{
		readonly dismiss: (id: string) => Effect.Effect<void>;
		readonly error: (title: string, options?: BannerOptions) => Effect.Effect<void>;
		readonly warning: (title: string, options?: BannerOptions) => Effect.Effect<void>;
		readonly info: (title: string, options?: BannerOptions) => Effect.Effect<void>;
		readonly success: (title: string, options?: BannerOptions) => Effect.Effect<void>;
	}
>()("Artisan/BannerService") {}

export const BannerReporterNoopLive = Layer.succeed(
	BannerReporter,
	BannerReporter.of({
		Report: () => Effect.gen(function* () {}),
	}),
);

export const BannerServiceLive = Layer.effect(
	BannerService,
	Effect.gen(function* () {
		const presenter = yield* BannerPresenter;
		const reporter = yield* BannerReporter;
		const actions = yield* Queue.unbounded<BannerExecutableAction>();
		/** The presenter callback is a foreign UI ingress; this scoped queue serializes only banner actions. */
		const PresentActions = Effect.gen(function* () {
			while (true) {
				const action = yield* Queue.take(actions);
				yield* action.Execute.pipe(Effect.exit, Effect.asVoid);
			}
		});
		yield* PresentActions.pipe(Effect.forkScoped);
		const on_action = (action: BannerExecutableAction) => {
			Queue.offerUnsafe(actions, action);
		};

		const Show = (severity: BannerSeverity, title: string, options?: BannerOptions) =>
			Effect.gen(function* () {
				const event: BannerEvent = { severity, title, ...options };
				const { actions: event_actions, ...report_fields } = event;
				const redacted_report_event: BannerReportEvent = {
					...report_fields,
					...(event_actions === undefined
						? {}
						: {
								actions: event_actions.map(({ icon, id, label }) => ({
									...(icon === undefined ? {} : { icon }),
									id,
									label,
								})),
							}),
				};
				yield* presenter.Show(event, on_action);
				yield* reporter.Report(redacted_report_event).pipe(Effect.ignore);
			});

		return BannerService.of({
			dismiss: (id) =>
				Effect.gen(function* () {
					yield* presenter.Dismiss(id);
				}),
			error: (title, options) =>
				Effect.gen(function* () {
					yield* Show("error", title, options);
				}),
			warning: (title, options) =>
				Effect.gen(function* () {
					yield* Show("warning", title, options);
				}),
			info: (title, options) =>
				Effect.gen(function* () {
					yield* Show("info", title, options);
				}),
			success: (title, options) =>
				Effect.gen(function* () {
					yield* Show("success", title, options);
				}),
		});
	}),
);
