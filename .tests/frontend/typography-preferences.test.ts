import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect, Schema } from "effect";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
	default_typography_preferences,
	resolve_typography_preferences,
	typography_css_stacks,
	TypographyFamily,
} from "../../modules/frontend/src/lib/appearance/typography";
import {
	BrowserTypography,
	BrowserTypographyLive,
	LocalFontData,
	local_font_family_limit,
	normalize_local_font_families,
} from "../../modules/frontend/src/lib/browser/typography";
import { AppearanceState } from "../../modules/frontend/src/lib/runtime/appearance-preferences";

const read_source = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("typography preferences", () => {
	afterEach(() => vi.unstubAllGlobals());

	it("keeps v1 appearance records valid and resolves bundled typography defaults", () => {
		const appearance = Schema.decodeUnknownSync(AppearanceState)({
			version: 1,
			shader_enabled: false,
		});

		expect(resolve_typography_preferences(appearance)).toEqual(default_typography_preferences);
		expect(default_typography_preferences).toEqual({
			text: "Artisan Neo",
			code: "JetBrains Mono",
		});
		const legacy_appearance = Schema.decodeUnknownSync(AppearanceState)({
			version: 1,
			shader_enabled: false,
			prose_width: "loose",
			typography: { sans: "Legacy Text", serif: "Ignored Serif", mono: "Legacy Code" },
		});
		expect(resolve_typography_preferences(legacy_appearance)).toEqual({
			text: "Legacy Text",
			code: "Legacy Code",
		});
		expect(legacy_appearance).toMatchObject({ shader_enabled: false, prose_width: "loose" });
	});

	it("canonicalizes bounded family names and builds escaped role-specific CSS stacks", () => {
		expect(Schema.decodeUnknownSync(TypographyFamily)('  A\\" Font  ')).toBe('A\\" Font');
		expect(() => Schema.decodeUnknownSync(TypographyFamily)("\u0000bad")).toThrow();

		const stacks = typography_css_stacks({
			text: 'A\\" Font',
			code: "JetBrains Mono",
		});
		expect(stacks["--font-sans"]).toBe('"A\\\\\\" Font", ui-sans-serif, system-ui, sans-serif');
		expect(stacks["--font-heading"]).toBe(stacks["--font-sans"]);
		expect(stacks["--font-mono"]).toContain("ui-monospace");
	});

	it("normalizes local families deterministically and bounds the selector inventory", () => {
		const decode = Schema.decodeUnknownSync(LocalFontData);
		const fonts = [
			decode({ family: "zebra" }),
			decode({ family: "Alpha" }),
			decode({ family: "alpha" }),
			...Array.from({ length: local_font_family_limit + 20 }, (_, index) =>
				decode({ family: `family-${index.toString().padStart(3, "0")}` }),
			),
		];

		const normalized = normalize_local_font_families(fonts);
		expect(normalized).toHaveLength(local_font_family_limit);
		expect(normalized.slice(0, 2)).toEqual(["Alpha", "family-000"]);
		expect(normalized).not.toContain("alpha");
		expect(() => decode({ family: "\u0000invalid" })).toThrow();
	});

	it("models unavailable local-font discovery as a tagged typed failure", async () => {
		vi.stubGlobal("queryLocalFonts", undefined);
		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					const typography = yield* BrowserTypography;
					return yield* typography.DiscoverLocalFonts;
				}).pipe(Effect.provide(BrowserTypographyLive)),
			),
		).rejects.toMatchObject({ _tag: "LocalFontsUnavailable" });
	});

	it("preserves the Window receiver and models denied access as a tagged failure", async () => {
		const query_local_fonts = vi.fn(function (this: unknown) {
			expect(this).toBe(globalThis);
			return Promise.reject(new DOMException("Permission denied", "NotAllowedError"));
		});
		vi.stubGlobal("queryLocalFonts", query_local_fonts);

		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					const typography = yield* BrowserTypography;
					return yield* typography.DiscoverLocalFonts;
				}).pipe(Effect.provide(BrowserTypographyLive)),
			),
		).rejects.toMatchObject({ _tag: "LocalFontsDenied" });
		expect(query_local_fonts).toHaveBeenCalledTimes(1);
	});

	it("applies every semantic role to the document root", async () => {
		const set_property = vi.fn();
		vi.stubGlobal("document", { documentElement: { style: { setProperty: set_property } } });

		await Effect.runPromise(
			Effect.gen(function* () {
				const typography = yield* BrowserTypography;
				yield* typography.Apply(default_typography_preferences);
			}).pipe(Effect.provide(BrowserTypographyLive)),
		);

		expect(set_property).toHaveBeenCalledWith(
			"--font-sans",
			'"Artisan Neo", ui-sans-serif, system-ui, sans-serif',
		);
		expect(set_property).toHaveBeenCalledWith(
			"--font-heading",
			expect.stringContaining('"Artisan Neo"'),
		);
		expect(set_property).toHaveBeenCalledWith(
			"--font-mono",
			expect.stringContaining('"JetBrains Mono"'),
		);
	});

	it("caches only a successful local-font enumeration for the service lifetime", async () => {
		const query_local_fonts = vi.fn(() => Promise.resolve([{ family: "Cache Face" }]));
		vi.stubGlobal("queryLocalFonts", query_local_fonts);
		const families = await Effect.runPromise(
			Effect.gen(function* () {
				const typography = yield* BrowserTypography;
				const first = yield* typography.DiscoverLocalFonts;
				const second = yield* typography.DiscoverLocalFonts;
				return [first, second] as const;
			}).pipe(Effect.provide(BrowserTypographyLive)),
		);
		expect(families).toEqual([["Cache Face"], ["Cache Face"]]);
		expect(query_local_fonts).toHaveBeenCalledTimes(1);
	});

	it("joins concurrent local-font discovery onto one browser permission request", async () => {
		let resolve_query!: (fonts: unknown) => void;
		const query_local_fonts = vi.fn(
			() =>
				new Promise<unknown>((resolve_query_promise) => {
					resolve_query = resolve_query_promise;
				}),
		);
		vi.stubGlobal("queryLocalFonts", query_local_fonts);

		const running = Effect.runPromise(
			Effect.gen(function* () {
				const typography = yield* BrowserTypography;
				return yield* Effect.all(
					[typography.DiscoverLocalFonts, typography.DiscoverLocalFonts],
					{ concurrency: "unbounded" },
				);
			}).pipe(Effect.provide(BrowserTypographyLive)),
		);
		await vi.waitFor(() => expect(query_local_fonts).toHaveBeenCalledTimes(1));
		resolve_query([{ family: "Shared Face" }]);

		await expect(running).resolves.toEqual([["Shared Face"], ["Shared Face"]]);
		expect(query_local_fonts).toHaveBeenCalledTimes(1);
	});

	it("does not cache a failed local-font request", async () => {
		const query_local_fonts = vi
			.fn<() => Promise<unknown>>()
			.mockRejectedValueOnce(new DOMException("Permission denied", "NotAllowedError"))
			.mockResolvedValueOnce([{ family: "Retry Face" }]);
		vi.stubGlobal("queryLocalFonts", query_local_fonts);

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const typography = yield* BrowserTypography;
				const first = yield* typography.DiscoverLocalFonts.pipe(Effect.result);
				const second = yield* typography.DiscoverLocalFonts;
				return { first, second };
			}).pipe(Effect.provide(BrowserTypographyLive)),
		);

		expect(result.first).toMatchObject({ _tag: "Failure" });
		expect(result.second).toEqual(["Retry Face"]);
		expect(query_local_fonts).toHaveBeenCalledTimes(2);
	});

	it("keeps queryLocalFonts behind a picker gesture rather than startup composition", () => {
		const service = read_source("modules/frontend/src/lib/browser/typography.ts");
		const runtime = read_source("modules/frontend/src/lib/runtime/browser-frontend-runtime.ts");
		const picker = read_source(
			"modules/frontend/src/routes/components/settings/font-picker.svelte",
		);
		const layout = read_source("modules/frontend/src/routes/+layout.svelte");

		expect(service).toContain("DiscoverLocalFonts = Effect.gen");
		expect(service).toContain("Effect.tryPromise");
		expect(runtime).toContain("BrowserTypographyLive");
		expect(runtime).not.toContain("queryLocalFonts");
		expect(picker).toContain("onclick={yield* DiscoverOnOpen}");
		expect(picker).toContain("browser_typography.DiscoverLocalFonts");
		expect(picker).toContain('aria-current="true"');
		expect(picker).toContain(
			'aria-current={candidate.toLowerCase() === family_key ? "true" : undefined}',
		);
		expect(layout).toContain("browser_typography.Apply(next_typography)");
		expect(layout).toContain("appearance.Load.pipe(Effect.forkScoped)");
		expect(layout).not.toContain("DiscoverLocalFonts");
	});
});
