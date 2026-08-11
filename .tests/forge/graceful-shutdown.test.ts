import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Ref } from "effect";
import { describe, expect, it } from "vitest";

import { MakeForgeClose } from "../../modules/forge/src/forge-host";

const entry_path = fileURLToPath(new URL("../../modules/forge/src/entry.ts", import.meta.url));

describe("Forge graceful shutdown", () => {
	it("drains concurrently and closes host resources once", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const primary_started = yield* Deferred.make<void>();
				const graph_started = yield* Deferred.make<void>();
				const release_drains = yield* Deferred.make<void>();
				const released = yield* Ref.make(0);
				const scope_closed = yield* Ref.make(0);
				const Close = yield* MakeForgeClose({
					close_scope: Ref.update(scope_closed, (count) => count + 1),
					drains: [
						Deferred.succeed(primary_started, undefined).pipe(
							Effect.andThen(Deferred.await(release_drains)),
						),
						Deferred.succeed(graph_started, undefined).pipe(
							Effect.andThen(Deferred.await(release_drains)),
						),
					],
					release_lease: Ref.update(released, (count) => count + 1),
				});

				const closing = yield* Close.pipe(Effect.forkChild);
				yield* Deferred.await(primary_started);
				yield* Deferred.await(graph_started);
				yield* Deferred.succeed(release_drains, undefined);
				yield* Fiber.join(closing);
				yield* Effect.all([Close, Close], { concurrency: "unbounded", discard: true });

				return {
					released: yield* Ref.get(released),
					scope_closed: yield* Ref.get(scope_closed),
				};
			}),
		);

		expect(result).toEqual({ released: 1, scope_closed: 1 });
	});

	it("interrupts a stalled drain at its shared deadline before closing resources", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const interrupted = yield* Ref.make(false);
				const released = yield* Ref.make(0);
				const scope_closed = yield* Ref.make(0);
				const Close = yield* MakeForgeClose({
					close_scope: Ref.update(scope_closed, (count) => count + 1),
					drains: [
						Effect.never.pipe(Effect.onInterrupt(() => Ref.set(interrupted, true))),
					],
					release_lease: Ref.update(released, (count) => count + 1),
					timeout: "10 millis",
				});
				yield* Close;

				return {
					interrupted: yield* Ref.get(interrupted),
					released: yield* Ref.get(released),
					scope_closed: yield* Ref.get(scope_closed),
				};
			}),
		);

		expect(result).toEqual({ interrupted: true, released: 1, scope_closed: 1 });
	});

	it("removes external state only after the graceful host close", async () => {
		const source = await readFile(entry_path, "utf8");
		const close = source.indexOf("yield* current_host.Close;");
		const remove = source.indexOf("yield* RemoveForgeState(state_path, instance_id);");

		expect(close).toBeGreaterThanOrEqual(0);
		expect(remove).toBeGreaterThan(close);
	});
});
