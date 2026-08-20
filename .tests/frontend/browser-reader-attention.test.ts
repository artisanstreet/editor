import { Effect, Fiber, Option, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	BrowserReaderAttention,
	BrowserReaderAttentionLive,
	ReaderIsWatching,
	reader_can_acknowledge_root_conversation,
} from "../../modules/frontend/src/lib/browser/reader-attention";

class FakeEventTarget implements Stream.EventListener<unknown> {
	private readonly listeners = new Map<string, Set<(event: unknown) => void>>();

	addEventListener(event: string, listener: (event: unknown) => void): void {
		const listeners = this.listeners.get(event) ?? new Set();
		listeners.add(listener);
		this.listeners.set(event, listeners);
	}

	emit(event: string): void {
		for (const listener of this.listeners.get(event) ?? []) listener({ type: event });
	}

	listenerCount(event: string): number {
		return this.listeners.get(event)?.size ?? 0;
	}

	removeEventListener(event: string, listener: (event: unknown) => void): void {
		this.listeners.get(event)?.delete(listener);
	}
}

class FakeDocument extends FakeEventTarget {
	focused = true;
	visibilityState = "visible";

	hasFocus(): boolean {
		return this.focused;
	}
}

const WithBrowserTargets = async <Value>(
	document: FakeDocument,
	window: FakeEventTarget,
	program: Effect.Effect<Value, never, BrowserReaderAttention>,
): Promise<Value> => {
	const previous_document = Object.getOwnPropertyDescriptor(globalThis, "document");
	const previous_window = Object.getOwnPropertyDescriptor(globalThis, "window");
	Object.defineProperty(globalThis, "document", { configurable: true, value: document });
	Object.defineProperty(globalThis, "window", { configurable: true, value: window });

	try {
		return await Effect.runPromise(program.pipe(Effect.provide(BrowserReaderAttentionLive)));
	} finally {
		if (previous_document === undefined) Reflect.deleteProperty(globalThis, "document");
		else Object.defineProperty(globalThis, "document", previous_document);
		if (previous_window === undefined) Reflect.deleteProperty(globalThis, "window");
		else Object.defineProperty(globalThis, "window", previous_window);
	}
};

describe("browser reader attention", () => {
	it("requires both focus and a visible document before activity counts as read", () => {
		expect(ReaderIsWatching(true, "visible")).toBe(true);
		expect(ReaderIsWatching(false, "visible")).toBe(false);
		expect(ReaderIsWatching(true, "hidden")).toBe(false);
	});

	it("does not acknowledge root activity while a worker transcript is selected", () => {
		expect(reader_can_acknowledge_root_conversation(true, false)).toBe(true);
		expect(reader_can_acknowledge_root_conversation(true, true)).toBe(false);
		expect(reader_can_acknowledge_root_conversation(false, false)).toBe(false);
	});

	it("publishes browser focus and visibility changes through one scoped signal", async () => {
		const document = new FakeDocument();
		const window = new FakeEventTarget();

		await WithBrowserTargets(
			document,
			window,
			Effect.gen(function* () {
				const attention = yield* BrowserReaderAttention;
				expect(yield* attention.Current).toBe(true);
				for (let attempt = 0; attempt < 10; attempt += 1) yield* Effect.yieldNow;
				expect(document.listenerCount("visibilitychange")).toBe(1);
				expect(window.listenerCount("focus")).toBe(1);
				expect(window.listenerCount("blur")).toBe(1);

				const hidden = yield* attention.Changes.pipe(
					Stream.filter((watching) => !watching),
					Stream.runHead,
					Effect.forkChild({ startImmediately: true }),
				);
				document.visibilityState = "hidden";
				document.emit("visibilitychange");
				expect(Option.getOrUndefined(yield* Fiber.join(hidden))).toBe(false);
				expect(yield* attention.Current).toBe(false);

				const visible = yield* attention.Changes.pipe(
					Stream.filter((watching) => watching),
					Stream.runHead,
					Effect.forkChild({ startImmediately: true }),
				);
				document.visibilityState = "visible";
				window.emit("focus");
				expect(Option.getOrUndefined(yield* Fiber.join(visible))).toBe(true);
				expect(yield* attention.Current).toBe(true);
			}),
		);
	});
});
