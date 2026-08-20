import { globSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";

import { LanguageVariant, SyntaxKind, createScanner } from "typescript/unstable/ast";
import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const source_root = resolve(workspace, "modules/frontend/src");
const vite_config_path = resolve(workspace, "modules/frontend/vite.config.ts");
const browser_dom_sources = [
	"modules/frontend/src/routes/components/command-menu.svelte",
	"modules/frontend/src/routes/components/forge-connection-overlay.svelte",
	"modules/frontend/src/routes/components/conversation-prompt.svelte",
	"modules/frontend/src/routes/settings/+layout.svelte",
	"modules/frontend/src/lib/components/ui/input-group/input-group-addon.svelte",
	"modules/frontend/src/lib/components/dropdown-highlight.ts",
	"modules/frontend/src/lib/components/activity/vertical-calendar-activity-grid.svelte",
	"modules/frontend/src/routes/components/conversation-message.svelte",
	"modules/frontend/src/routes/components/conversation-work-session.svelte",
	"modules/frontend/src/routes/components/dev-instance-badge.svelte",
	"modules/frontend/src/routes/components/model-selector/engine-section.svelte",
	"modules/frontend/src/routes/components/paper-god-rays.svelte",
	"modules/frontend/src/routes/components/thread-hover-rail.svelte",
	"modules/frontend/src/routes/components/thread-workspace.svelte",
] as const;

/**
 * These modules are the narrow, typed owners of browser capabilities. Every
 * other application source file must yield host work through RunBrowserDom or
 * RunAdapter so a new component cannot create an unreviewed host boundary.
 */
const typed_browser_boundary_modules = new Set([
	"modules/frontend/src/lib/browser/clipboard.ts",
	"modules/frontend/src/lib/browser/dom.ts",
	"modules/frontend/src/lib/browser/object-url.ts",
	"modules/frontend/src/lib/browser/typography.ts",
	"modules/frontend/src/lib/composer/attachment-reader.ts",
	/** Owns the gestures whose DOM contract expires with their own dispatch. */
	"modules/frontend/src/lib/composer/gesture-intake.ts",
	/** Owns the canvas decode/re-encode that shrinks a pasted image. */
	"modules/frontend/src/lib/composer/image-encoding.ts",
	"modules/frontend/src/lib/editor/codemirror-adapter.ts",
	"modules/frontend/src/lib/runtime/forge-endpoint.ts",
]);

const browser_host_operation =
	/(?:(?<![A-Za-z0-9_$.])(?:document|navigator|window|URL)\s*(?:\.|\[)|\bglobalThis\.(?!atob\b)|\bevent\.(?:preventDefault|stopPropagation)|\b(?:requestAnimationFrame|cancelAnimationFrame|getComputedStyle)\(|\bnew\s+(?:ResizeObserver|MutationObserver|IntersectionObserver)|\.(?:focus|blur|select|scrollIntoView|getBoundingClientRect|querySelector(?:All)?|closest|appendChild|removeChild|replaceChildren|observe|disconnect)\(|\.(?:classList\.(?:add|remove|toggle)|style\.(?:setProperty|removeProperty))\(|\.(?:deleteContents|extractContents|insertNode|setStart|setEnd|selectNode(?:Contents)?)\()/gu;

const non_executable_tokens = new Set([
	SyntaxKind.SingleLineCommentTrivia,
	SyntaxKind.MultiLineCommentTrivia,
	SyntaxKind.WhitespaceTrivia,
	SyntaxKind.NewLineTrivia,
	SyntaxKind.StringLiteral,
	SyntaxKind.NoSubstitutionTemplateLiteral,
	SyntaxKind.TemplateHead,
	SyntaxKind.TemplateMiddle,
	SyntaxKind.TemplateTail,
	SyntaxKind.RegularExpressionLiteral,
]);

/**
 * Preserves source offsets while removing comments and literals. Boundary
 * recognition must inspect executable tokens: raw-text parenthesis counting
 * lets a comment or string forge a fake `RunBrowserDom` call around later code.
 */
const ExecutableSource = (source: string): string => {
	const lexical_source = source.replace(/<!--[\s\S]*?-->/gu, (comment) =>
		comment.replace(/[^\r\n]/gu, " "),
	);
	const executable = source
		.split("")
		.map<string>((character) => (character === "\r" || character === "\n" ? character : " "));
	const scanner = createScanner(false, LanguageVariant.Standard, lexical_source);
	let previous_end = -1;
	while (true) {
		const token = scanner.scan();
		if (token === SyntaxKind.EndOfFile) break;
		const start = scanner.getTokenStart();
		const end = scanner.getTokenEnd();
		/** Svelte template syntax can be invalid to the TypeScript-only scanner. */
		if (end <= previous_end) {
			scanner.resetTokenState(Math.min(source.length, previous_end + 1));
			continue;
		}
		previous_end = end;
		if (non_executable_tokens.has(token)) continue;
		for (let index = start; index < end; index += 1) executable[index] = source[index]!;
	}

	return executable.join("");
};

/**
 * SER is the only application Effect executor. This deliberately scans source
 * rather than runtime output: a source-level regression must fail before a
 * transform can hide its lifecycle boundary.
 */
const universal_rules: ReadonlyArray<readonly [string, RegExp]> = [
	[
		"manual Effect executor",
		/\bEffect\.run(?:Sync|Promise|Fork|Callback|SyncExit|PromiseExit)\b/gu,
	],
	["manual Runtime executor", /\bRuntime\.run[A-Za-z]*\b/gu],
	["competing managed runtime", /\bManagedRuntime\.make\b/gu],
	[
		"Promise control flow",
		/\basync\s*(?:function\b|\([^)]*\)\s*=>|[A-Za-z_$][A-Za-z0-9_$]*\s*=>)|(?<!\.)\bawait\b|\.then\s*\(|\bnew Promise\b/gu,
	],
	["onMount lifecycle executor", /\bonMount\s*\(/gu],
	["discarded browser Promise", /\bvoid\s+navigator(?:\.[A-Za-z_$][A-Za-z0-9_$]*)+\s*\(/gu],
	["unnamed forever worker root", /\byield\*\s*Effect\.forever\s*\(/gu],
];

/**
 * SER owns component execution. App-scoped Layer services compose Effect
 * programs deliberately, but components must keep every such program behind a
 * yielded generator boundary so reactive reruns retain one explicit owner.
 */
const ser_component_rules: ReadonlyArray<readonly [string, RegExp]> = [
	["Effect.void camouflage", /\bEffect\.void\b/gu],
	["non-generator Effect workflow", /\bEffect\.(?:flatMap|andThen|sync|succeed|suspend)\b/gu],
	[
		"direct non-generator Effect return",
		/=>\s*(?:\r?\n\s*)?Effect\.(?!gen\b|Effect\b)[A-Za-z0-9_$]+/gu,
	],
	[
		"direct Effect pipeline return",
		/=>\s*(?:\r?\n\s*)?[A-Za-z_$][A-Za-z0-9_$]*(?:\.[A-Za-z_$][A-Za-z0-9_$]*)?\.pipe\s*\(/gu,
	],
	[
		"non-generator Effect program binding",
		/\b(?:const|let)\s+[A-Za-z_$][A-Za-z0-9_$]*\s*=\s*Effect\.(?!gen\b|Effect\b)[A-Za-z0-9_$]+/gu,
	],
	[
		"direct service Effect return",
		/=>\s*(?:\r?\n\s*)?(?:client|service|editor)\.[A-Z][A-Za-z0-9_]*/gu,
	],
	[
		"conditional non-generator Effect helper",
		/=>\s*(?:\r?\n\s*)?[\s\S]{0,160}?\?\s*[A-Za-z_$][A-Za-z0-9_$]*\([^;\n]*\)\s*:\s*Effect\.gen\s*\(/gu,
	],
];

/**
 * These APIs are operators only when they occur inside an Effect pipeline.
 * Every other Effect member is a program constructor/value and must be yielded
 * from an Effect.gen boundary. Keeping this as an allow-list makes newly used
 * Effect APIs fail closed instead of silently opening another gate blind spot.
 */
const pipe_operators = new Set([
	"asVoid",
	"catch",
	"catchTag",
	"ensuring",
	"exit",
	"filterOrFail",
	"forkIn",
	"forkScoped",
	"ignore",
	"map",
	"mapError",
	"matchEffect",
	"option",
	"provide",
	"result",
	"retry",
	"tap",
	"tapError",
	"timeoutOption",
]);

const IsInsidePipe = (source: string, offset: number): boolean => {
	let pipe_offset = source.lastIndexOf(".pipe(", offset);
	while (pipe_offset >= 0) {
		let depth = 0;
		for (let index = pipe_offset + ".pipe".length; index < offset; index += 1) {
			const character = source[index];
			if (character === "(") depth += 1;
			else if (character === ")") depth -= 1;
		}
		if (depth > 0) return true;
		pipe_offset = source.lastIndexOf(".pipe(", pipe_offset - 1);
	}
	return false;
};

/** A Promise is allowed only as the callback owned by Effect.tryPromise. */
const IsInsideTryPromise = (source: string, offset: number): boolean => {
	const boundary = source.lastIndexOf("Effect.tryPromise(", offset);
	if (boundary < 0) return false;
	let depth = 0;
	for (let index = boundary + "Effect.tryPromise".length; index < offset; index += 1) {
		const character = source[index];
		if (character === "(") depth += 1;
		else if (character === ")") depth -= 1;
	}
	return depth > 0;
};

const UnwrappedEffectPrograms = (source: string): ReadonlyArray<readonly [number, string]> => {
	const violations: Array<readonly [number, string]> = [];
	for (const match of source.matchAll(/\bEffect\.([A-Za-z_$][A-Za-z0-9_$]*)/gu)) {
		const offset = match.index ?? 0;
		const member = match[1];
		if (member === undefined || member === "Effect" || member === "gen") continue;
		if (/yield\*\s*$/u.test(source.slice(Math.max(0, offset - 32), offset))) continue;
		if (pipe_operators.has(member) && IsInsidePipe(source, offset)) continue;
		violations.push([offset, match[0]]);
	}
	return violations;
};

const effectful_capability_member =
	/\b(Clock\.currentTimeMillis|Fiber\.interrupt|Queue\.(?:offer|take|unbounded)|Ref\.(?:get|make|modify|set|update|updateAndGet)|Schema\.decodeUnknownEffect|Scope\.(?:addFinalizer|close|make)|Semaphore\.make|Stream\.runForEach|SubscriptionRef\.(?:get|make|set|update|updateAndGet))\b/gu;

const UnwrappedCapabilityPrograms = (source: string): ReadonlyArray<readonly [number, string]> => {
	const violations: Array<readonly [number, string]> = [];
	for (const match of source.matchAll(effectful_capability_member)) {
		const offset = match.index ?? 0;
		if (/yield\*\s*$/u.test(source.slice(Math.max(0, offset - 32), offset))) continue;
		if (match[0] === "Stream.runForEach" && IsInsidePipe(source, offset)) continue;
		violations.push([offset, match[0]]);
	}
	return violations;
};

/**
 * The legacy repository gate covered TypeScript only (`.sv` matched no Artisan
 * sources). Expand Svelte coverage deliberately as components are brought
 * under the scanner instead of making unrelated existing debt fail this slice.
 */
const audited_svelte_sources = [
	resolve(source_root, "routes/components/settings/engine.svelte"),
] as const;

const SourcePaths = () => [
	...globSync("**/*.ts", {
		cwd: source_root,
	}).map((path) => resolve(source_root, path)),
	...audited_svelte_sources,
];

const IsInsideBrowserBoundary = (source: string, offset: number): boolean => {
	for (const match of source.matchAll(
		/\b(?:RunBrowserDom|RunAdapter)\s*\(|\bEffect\.callback(?:<[^>]+>)?\s*\(/gu,
	)) {
		const opening_parenthesis = source.indexOf("(", match.index);
		if (opening_parenthesis < 0 || opening_parenthesis >= offset) continue;

		let depth = 0;
		for (let index = opening_parenthesis; index < offset; index += 1) {
			const character = source[index];
			if (character === "(") depth += 1;
			else if (character === ")") depth -= 1;
		}
		if (depth > 0) return true;
	}
	return false;
};

const BrowserHostViolations = (path: string, source: string): ReadonlyArray<string> => {
	if (typed_browser_boundary_modules.has(path)) return [];

	const executable = ExecutableSource(source);
	const violations: Array<string> = [];
	for (const match of executable.matchAll(browser_host_operation)) {
		const offset = match.index ?? 0;
		if (IsInsideBrowserBoundary(executable, offset)) continue;
		violations.push(
			`${path}:${LineForOffset(source, offset)}: browser host operation outside a typed boundary: ${match[0]}`,
		);
	}
	return violations;
};

/**
 * `offerUnsafe` is permitted only as synchronous ingress from host callbacks
 * whose same module owns a generator Queue.take worker. Exact counts make the
 * exceptions reviewable: another ingress, even in an allowed file, fails until
 * its lifecycle ownership is deliberately audited here.
 */
const unsafe_queue_ingress = new Map<string, number>([
	["modules/frontend/src/lib/components/dropdown-highlight.ts", 4],
	["modules/frontend/src/lib/composer/gesture-intake.ts", 2],
	["modules/frontend/src/lib/lifecycle/scoped-attachment-runner.ts", 3],
	/** The host invokes a notification's click handler on its own callback. */
	["modules/frontend/src/lib/notifications/service.ts", 1],
	["modules/frontend/src/routes/components/paper-god-rays.svelte", 3],
	["modules/frontend/src/routes/components/dev-instance-badge.svelte", 1],
	["modules/frontend/src/routes/components/thread-workspace.svelte", 2],
]);

const UnsafeQueueViolations = (path: string, source: string): ReadonlyArray<string> => {
	const count = source.match(/\bQueue\.offerUnsafe\s*\(/gu)?.length ?? 0;
	if (count === 0) return [];
	const expected = unsafe_queue_ingress.get(path);
	if (expected === undefined) return [`unapproved Queue.offerUnsafe ingress (${count})`];
	if (count !== expected)
		return [
			`Queue.offerUnsafe ingress count changed (expected ${expected}, received ${count})`,
		];
	if (!/yield\*\s*Queue\.take\s*\(/u.test(source))
		return ["Queue.offerUnsafe boundary has no yielded Queue.take worker"];
	return [];
};

const LineForOffset = (source: string, offset: number) =>
	source.slice(0, offset).split(/\r?\n/u).length;

const ViolationsFromSource = (
	source: string,
	rules: ReadonlyArray<readonly [string, RegExp]> = universal_rules,
): ReadonlyArray<string> => {
	const violations: Array<string> = [];
	const executable = ExecutableSource(source);
	for (const [rule, expression] of rules) {
		for (const match of executable.matchAll(expression)) {
			if (
				rule === "Promise control flow" &&
				IsInsideTryPromise(executable, match.index ?? 0)
			) {
				continue;
			}
			violations.push(`${rule}: ${match[0]}`);
		}
	}
	return violations;
};

const Violations = () => {
	const violations: Array<string> = [];
	for (const path of SourcePaths()) {
		const source = readFileSync(path, "utf8");
		const relative_path = relative(workspace, path).replaceAll("\\", "/");
		for (const violation of ViolationsFromSource(source)) {
			violations.push(
				`${relative_path}:${LineForOffset(source, source.indexOf(violation.split(": ")[1] ?? ""))}: ${violation}`,
			);
		}
		if (audited_svelte_sources.includes(path as (typeof audited_svelte_sources)[number])) {
			for (const violation of ViolationsFromSource(source, ser_component_rules)) {
				violations.push(
					`${relative_path}:${LineForOffset(source, source.indexOf(violation.split(": ")[1] ?? ""))}: ${violation}`,
				);
			}
			for (const [offset, program] of UnwrappedEffectPrograms(source)) {
				violations.push(
					`${relative(workspace, path)}:${LineForOffset(source, offset)}: Effect program outside yielded generator boundary: ${program}`,
				);
			}
			for (const [offset, program] of UnwrappedCapabilityPrograms(source)) {
				violations.push(
					`${relative(workspace, path)}:${LineForOffset(source, offset)}: Effect capability program outside yielded generator boundary: ${program}`,
				);
			}
		}
		for (const violation of UnsafeQueueViolations(relative_path, source)) {
			violations.push(`${relative_path}: ${violation}`);
		}
		violations.push(...BrowserHostViolations(relative_path, source));
	}
	return violations.sort();
};

describe("Svelte Effect Runtime source discipline", () => {
	it("keeps application Effect execution and workflows inside SER generator boundaries", () => {
		expect(Violations()).toEqual([]);
	});

	it("includes Settings components in the SER source audit", () => {
		expect(SourcePaths()).toContain(
			resolve(source_root, "routes/components/settings/engine.svelte"),
		);
	});

	it("fails closed for direct constructor arguments and service property values", () => {
		expect(ViolationsFromSource("yield* Deferred.await(completed)")).toEqual([]);
		expect(ViolationsFromSource("const value = await promise")).toEqual([
			"Promise control flow: await",
		]);
		expect(
			UnwrappedEffectPrograms("Layer.effect(Tag, Effect.acquireRelease(acquire, release))"),
		).toEqual([[18, "Effect.acquireRelease"]]);
		expect(UnwrappedEffectPrograms("({ Connect: Effect.fail(error) })")).toEqual([
			[12, "Effect.fail"],
		]);
		expect(UnwrappedEffectPrograms("resume(Effect.void)")).toEqual([[7, "Effect.void"]]);
		expect(UnwrappedCapabilityPrograms("({ Current: SubscriptionRef.get(state) })")).toEqual([
			[12, "SubscriptionRef.get"],
		]);
		expect(
			UnsafeQueueViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"Queue.offerUnsafe(queue, value)",
			),
		).toEqual(["unapproved Queue.offerUnsafe ingress (1)"]);
		expect(
			ViolationsFromSource(
				"const Apply = (update: Update) => update.ok ? Refresh() : Effect.gen(function* () {});",
				ser_component_rules,
			),
		).toEqual(
			expect.arrayContaining([
				expect.stringContaining("conditional non-generator Effect helper"),
			]),
		);
		expect(
			ViolationsFromSource(
				"Effect.tryPromise({ try: async () => await import('./renderer') })",
			),
		).toEqual([]);
	});

	it("fails closed for browser host operations outside an explicit boundary", () => {
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"const Focus = () => element.focus();",
			),
		).toEqual([
			"modules/frontend/src/routes/components/example.svelte:1: browser host operation outside a typed boundary: .focus(",
		]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"const Focus = () => RunBrowserDom(() => element.focus());",
			),
		).toEqual([]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"const Frame = Effect.callback(() => requestAnimationFrame(callback));",
			),
		).toEqual([]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"const Copy = () => navigator.clipboard.writeText(value);",
			),
		).toEqual([
			"modules/frontend/src/routes/components/example.svelte:1: browser host operation outside a typed boundary: navigator.",
		]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"const Select = () => document.getSelection();",
			),
		).toEqual([
			"modules/frontend/src/routes/components/example.svelte:1: browser host operation outside a typed boundary: document.",
		]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				"// RunBrowserDom(() => (\nconst Copy = () => navigator.clipboard.writeText(value);",
			),
		).toEqual([
			"modules/frontend/src/routes/components/example.svelte:2: browser host operation outside a typed boundary: navigator.",
		]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/routes/components/example.svelte",
				'const decoy = "RunBrowserDom(() => (";\nwindow.location.assign(path);',
			),
		).toEqual([
			"modules/frontend/src/routes/components/example.svelte:2: browser host operation outside a typed boundary: window.",
		]);
		expect(
			BrowserHostViolations(
				"modules/frontend/src/lib/browser/dom.ts",
				"const Read = () => document.activeElement;",
			),
		).toEqual([]);
	});

	it("permits generator roots, yielded children, and sanctioned pipe operators", () => {
		const source = `
			const Program = Effect.gen(function* () {
				yield* Effect.acquireRelease(acquire, release)
				return yield* Effect.fail(error)
			}).pipe(Effect.catch(() => Effect.gen(function* () {})))
		`;
		expect(UnwrappedEffectPrograms(source)).toEqual([]);
		expect(UnwrappedCapabilityPrograms("yield* Ref.get(state)")).toEqual([]);
	});

	it("documents Tailwind's CSS-only pre-transform exception to SER ordering notices", () => {
		const source = readFileSync(vite_config_path, "utf8");

		expect(source).toContain('Tailwind 4\'s build plugin is deliberately `enforce: "pre"`');
		expect(source).toContain("transform filter accepts CSS");
		expect(source).toContain("tailwindcss(),");
	});

	it.each(browser_dom_sources)(
		"routes browser-DOM work in %s through the tagged boundary",
		(path) => {
			const source = readFileSync(resolve(workspace, path), "utf8");

			expect(source).toContain('import { RunBrowserDom } from "$lib/browser/dom";');
			expect(source).not.toMatch(
				/on(?:click|submit|keydown|OpenAutoFocus|CloseAutoFocus)=\{\([^)]*\)\s*=>/u,
			);
			expect(source).not.toMatch(/\bevent\.(?:preventDefault|stopPropagation)\(\);/u);
			expect(source).not.toMatch(/\byield\*\s+Effect\.void\b/u);
		},
	);
});
