import { globSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const source_root = resolve(workspace, "modules/frontend/src");

/**
 * Declarations the SER transform exposes as reactive inputs. A yield site that
 * names one of these collects it as a dependency, so writing it from inside the
 * same site re-invalidates that site.
 */
const state_declaration = /\blet\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^=;]*)?=\s*\$state\b/gu;

const StateNames = (source: string): ReadonlySet<string> => {
	const names = new Set<string>();
	for (const match of source.matchAll(state_declaration)) {
		const name = match[1];
		if (name !== undefined) names.add(name);
	}
	return names;
};

/**
 * Blanks comments and literal text while preserving every offset, so brace
 * counting cannot be thrown off by a brace inside a string or a comment.
 */
const Masked = (source: string): string => {
	const characters = source.split("");
	let index = 0;
	const Blank = (start: number, end: number) => {
		for (let cursor = start; cursor < end && cursor < characters.length; cursor += 1) {
			if (characters[cursor] !== "\n" && characters[cursor] !== "\r")
				characters[cursor] = " ";
		}
	};
	while (index < source.length) {
		const rest = source.slice(index, index + 2);
		if (rest === "//") {
			const end = source.indexOf("\n", index);
			Blank(index, end < 0 ? source.length : end);
			index = end < 0 ? source.length : end;
			continue;
		}
		if (rest === "/*") {
			const end = source.indexOf("*/", index + 2);
			Blank(index, end < 0 ? source.length : end + 2);
			index = end < 0 ? source.length : end + 2;
			continue;
		}
		const character = source[index];
		if (character === '"' || character === "'" || character === "`") {
			let cursor = index + 1;
			while (cursor < source.length) {
				if (source[cursor] === "\\") cursor += 2;
				else if (source[cursor] === character) break;
				else cursor += 1;
			}
			Blank(index, Math.min(cursor + 1, source.length));
			index = cursor + 1;
			continue;
		}
		index += 1;
	}
	return characters.join("");
};

/** The `<script>` body, where SER compiles a top-level `yield*` into an effect. */
const ScriptBody = (source: string): { readonly offset: number; readonly text: string } => {
	const open = /<script\b[^>]*>/u.exec(source);
	if (open === null) return { offset: 0, text: source };
	const start = open.index + open[0].length;
	const end = source.indexOf("</script>", start);
	return { offset: start, text: source.slice(start, end < 0 ? source.length : end) };
};

/**
 * A `yield*` remains reactive inside top-level control flow such as `if` and
 * `while`; only function bodies leave the component's reactive scope. A
 * `yield*` inside `Effect.gen(function* () { … })` therefore belongs to the
 * program the component builds and runs deliberately, so it collects nothing.
 */
const TopLevelYieldOffsets = (body: string): ReadonlyArray<number> => {
	const masked = Masked(body);
	const offsets: Array<number> = [];
	const blocks: Array<boolean> = [];
	let expression_depth = 0;
	let function_depth = 0;
	const FunctionBlockAt = (index: number): boolean => {
		const prefix = masked.slice(0, index);
		return (
			/(?:\bfunction\s*\*?\s*[A-Za-z_$][A-Za-z0-9_$]*\s*|\bfunction\s*\*?\s*)\([^{}]*\)\s*$/u.test(
				prefix,
			) || /(?:\([^{}]*\)|[A-Za-z_$][A-Za-z0-9_$]*)\s*=>\s*$/u.test(prefix)
		);
	};
	for (let index = 0; index < masked.length; index += 1) {
		const character = masked[index];
		if (character === "(" || character === "[") {
			expression_depth += 1;
		} else if (character === ")" || character === "]") {
			expression_depth -= 1;
		} else if (character === "{") {
			const function_block = FunctionBlockAt(index);
			blocks.push(function_block);
			if (function_block) function_depth += 1;
		} else if (character === "}") {
			if (blocks.pop()) function_depth -= 1;
		} else if (
			function_depth === 0 &&
			expression_depth === 0 &&
			masked.startsWith("yield*", index)
		) {
			offsets.push(index);
		}
	}
	return offsets;
};

/** The text of one statement, from `yield*` to the `;` that closes it. */
const YieldStatement = (body: string, offset: number): string => {
	let depth = 0;
	for (let index = offset; index < body.length; index += 1) {
		const character = body[index];
		if (character === "(" || character === "[" || character === "{") depth += 1;
		else if (character === ")" || character === "]" || character === "}") depth -= 1;
		else if (character === ";" && depth === 0) return body.slice(offset, index);
	}
	return body.slice(offset);
};

/**
 * A subscription that writes the state its own yield site reads is an unbounded
 * loop, not a one-time wiring. The controller `Changes` streams in this app are
 * built from `SubscriptionRef.changes`, which replays the current value to each
 * new subscriber: the write invalidates the site, the site resubscribes, the
 * replay writes again. Every turn forks a scoped fiber and registers a PubSub
 * subscription against a component scope that outlives it, so the renderer leaks
 * roughly a megabyte a second and burns a core until it exhausts its heap.
 *
 * The remedy is the shape `+layout.svelte` already uses: bind the handler to a
 * plain const and yield only that name, so the state never appears in the yield
 * site's collected identifiers.
 */
export const ReactiveSubscriptionLoops = (path: string, source: string): ReadonlyArray<string> => {
	const names = StateNames(source);
	if (names.size === 0) return [];
	const body = ScriptBody(source);
	const violations: Array<string> = [];
	for (const offset of TopLevelYieldOffsets(body.text)) {
		const statement = YieldStatement(body.text, offset);
		/**
		 * A reactive site can leak only when it starts a scoped child whose callback
		 * writes state or constructs its own generator. Yielding a previously bound
		 * program is safe: its closure runs under the program's deliberate lifecycle,
		 * not SER's reactive site.
		 */
		if (
			!statement.includes("Effect.forkScoped") ||
			(!statement.includes("=>") && !statement.includes("function*"))
		)
			continue;
		for (const name of names) {
			if (!new RegExp(`\\b${name}\\s*=(?!=)`, "u").test(statement)) continue;
			const line = source.slice(0, body.offset + offset).split(/\r?\n/u).length;
			violations.push(
				`${path}:${line}: yield site writes the reactive state it reads (${name}); ` +
					`bind the handler to a const so the site stops depending on it`,
			);
		}
	}
	return violations;
};

const ComponentPaths = () =>
	globSync("**/*.svelte", { cwd: source_root }).map((path) => resolve(source_root, path));

describe("SER reactive subscription loops", () => {
	it("never writes a yield site's own reactive input from inside that site", () => {
		const violations: Array<string> = [];
		for (const path of ComponentPaths()) {
			const relative_path = relative(workspace, path).replaceAll("\\", "/");
			violations.push(
				...ReactiveSubscriptionLoops(relative_path, readFileSync(path, "utf8")),
			);
		}

		expect(violations.sort()).toEqual([]);
	});

	it("scans every component, so a retired file extension cannot silence it", () => {
		expect(ComponentPaths().length).toBeGreaterThan(200);
	});

	it("catches the inline handler that leaked the renderer", () => {
		const source = [
			'<script lang="ts" effect>',
			"\tlet refreshing_engines = $state.raw<ReadonlySet<string>>(yield* controller.Current);",
			"\tyield* controller.Changes.pipe(",
			"\t\tStream.runForEach((next) =>",
			"\t\t\tEffect.gen(function* () {",
			"\t\t\t\trefreshing_engines = next;",
			"\t\t\t}),",
			"\t\t),",
			"\t\tEffect.forkScoped,",
			"\t);",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([
			"example.svelte:3: yield site writes the reactive state it reads (refreshing_engines); " +
				"bind the handler to a const so the site stops depending on it",
		]);
	});

	it("catches a directly yielded clock generator that reforks itself on every tick", () => {
		const source = [
			'<script lang="ts" effect>',
			"\tlet now_ms = $state(Date.now());",
			"\tyield* Effect.gen(function* () {",
			"\t\twhile (live) {",
			'\t\t\tyield* Effect.sleep("1 second");',
			"\t\t\tnow_ms = yield* Clock.currentTimeMillis;",
			"\t\t}",
			"\t}).pipe(Effect.forkScoped);",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([
			"example.svelte:3: yield site writes the reactive state it reads (now_ms); " +
				"bind the handler to a const so the site stops depending on it",
		]);
	});

	it("catches an inline ticking generator below top-level control flow", () => {
		const source = [
			'<script lang="ts" effect>',
			"\tlet now_ms = $state(Date.now());",
			"\tif (live) {",
			"\t\tyield* Effect.gen(function* () {",
			'\t\t\tyield* Effect.sleep("1 second");',
			"\t\t\tnow_ms = yield* Clock.currentTimeMillis;",
			"\t\t}).pipe(Effect.forkScoped);",
			"\t}",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([
			"example.svelte:4: yield site writes the reactive state it reads (now_ms); " +
				"bind the handler to a const so the site stops depending on it",
		]);
	});

	it("permits a bound ticker below top-level control flow", () => {
		const source = [
			'<script lang="ts" effect>',
			"\tlet now_ms = $state(Date.now());",
			"\tconst KeepNowCurrent = Effect.gen(function* () {",
			'\t\tyield* Effect.sleep("1 second");',
			"\t\tnow_ms = yield* Clock.currentTimeMillis;",
			"\t});",
			"\tif (live) yield* KeepNowCurrent.pipe(Effect.forkScoped);",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([]);
	});

	it("permits a handler bound to a const", () => {
		const source = [
			'<script lang="ts" effect>',
			"\tlet refreshing_engines = $state.raw<ReadonlySet<string>>(yield* controller.Current);",
			"\tconst ApplyRefreshingEngines = (next: ReadonlySet<string>) =>",
			"\t\tEffect.gen(function* () {",
			"\t\t\trefreshing_engines = next;",
			"\t\t});",
			"\tyield* controller.Changes.pipe(",
			"\t\tStream.runForEach(ApplyRefreshingEngines),",
			"\t\tEffect.forkScoped,",
			"\t);",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([]);
	});

	it("ignores a write inside a program the component builds for itself", () => {
		const source = [
			'<script lang="ts" effect>',
			'\tlet copy_state = $state<CopyState>("idle");',
			"\tconst CopyCode = Effect.gen(function* () {",
			"\t\tyield* WriteClipboardText(text).pipe(",
			'\t\t\tEffect.catchTag("ClipboardWriteError", () =>',
			"\t\t\t\tEffect.gen(function* () {",
			'\t\t\t\t\tcopy_state = "failure";',
			"\t\t\t\t}),",
			"\t\t\t),",
			"\t\t);",
			"\t});",
			"</script>",
		].join("\n");

		expect(ReactiveSubscriptionLoops("example.svelte", source)).toEqual([]);
	});
});
