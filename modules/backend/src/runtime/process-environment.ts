import { Context, Effect, Layer } from "effect";

/** Supplies inherited environment variables to shell-free process adapters. */
export class ProcessEnvironment extends Context.Service<
	ProcessEnvironment,
	{
		readonly Variables: Effect.Effect<Readonly<Record<string, string | undefined>>>;
	}
>()("Artisan/ProcessEnvironment") {}

/** Reads Node's ambient environment only at the platform capability boundary. */
export const ProcessEnvironmentLive = Layer.succeed(ProcessEnvironment, {
	Variables: Effect.sync(() => ({ ...process.env })),
});
