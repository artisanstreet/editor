import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Option } from "effect";
import { describe, expect, it } from "vitest";

import { rich_link_metadata_url } from "../../modules/frontend/src/lib/components/markdown/link-url";

import { ReadStylesheets } from "./stylesheet-source";

const ReadSource = (path: string) => readFileSync(resolve(path), "utf8");

describe("conversation link presentation", () => {
	it("renders resolved page titles with retained favicon assets", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.svelte");
		const runtime = ReadSource("modules/frontend/src/lib/runtime/browser-frontend-runtime.ts");
		const links = ReadStylesheets();

		expect(anchor).toContain('<script lang="ts" effect>');
		expect(anchor).toContain("RichLinkMetadataController");
		expect(anchor).toContain("rich_link_metadata.Load(url)");
		expect(anchor).not.toContain("client.ResolveRichLink");
		expect(runtime).toContain("RichLinkMetadataControllerLive");
		expect(anchor).toContain("resolved_title = Option.some(resolution.value.page_name)");
		expect(anchor).toContain("{resolved_title.value}");
		expect(anchor).toContain("{@render children?.()}");
		expect(anchor).toContain("rich_link_assets.Load(favicon)");
		expect(anchor).toContain('Effect.timeoutOption("2 seconds")');
		expect(anchor).toContain("CreateBrowserObjectUrl(asset.bytes, asset.content_type)");
		expect(anchor).toContain("ReleaseBrowserObjectUrl(source)");
		expect(anchor).toContain("Effect.addFinalizer");
		expect(anchor).toContain("generation !== rich_link_generation");
		expect(anchor).toContain("Effect.uninterruptible");
		expect(anchor).toContain('class="conversation-link-favicon"');
		expect(anchor).toContain(".conversation-link-favicon {");
		expect(anchor).toContain("display: block;");
		expect(anchor).toContain("flex: none;");
		expect(anchor).toContain('World from "@tabler/icons-svelte/icons/world"');
		expect(anchor).toContain('class="conversation-link-web-fallback"');
		expect(anchor).toContain(".conversation-link-web-fallback {");
		expect(anchor).toContain("display: inline-flex;");
		expect(anchor).toContain("show_web_fallback = url !== undefined");
		expect(anchor).toContain("show_web_fallback = true");
		/** Both metadata icons stay in-flow and do not inherit prose image margins. */
		expect(anchor).toContain("margin-block: 0;");
		expect(anchor).toContain("margin-inline: 0;");
		expect(anchor).toContain('alt=""');
		expect(anchor).toContain("onerror={yield* HideFailedFavicon(favicon_source.value)}");
		expect(anchor).toContain("title={safe_href}");
		expect(anchor).not.toContain("rich_link_debug_status");
		expect(links).toContain("a.conversation-link");
		expect(links).toContain("display: inline-flex;");
		/** Icons seat on the line box, the link itself on the prose baseline. */
		expect(anchor).toContain("align-self: center;");
		expect(links).toContain("align-items: baseline;");
		expect(links).toContain("gap: 0.375em;");
		expect(links).toContain("vertical-align: baseline;");
		expect(links).toContain("text-blue-500 no-underline");
		expect(links).toContain("dark:text-blue-400");
	});

	it("resolves favicon metadata only for absolute HTTP(S) destinations", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.svelte");

		expect(Option.getOrUndefined(rich_link_metadata_url("https://example.com/docs"))).toBe(
			"https://example.com/docs",
		);
		expect(Option.getOrUndefined(rich_link_metadata_url("http://example.com"))).toBe(
			"http://example.com/",
		);
		expect(Option.isNone(rich_link_metadata_url("mailto:hello@example.com"))).toBe(true);
		expect(Option.isNone(rich_link_metadata_url("/relative"))).toBe(true);
		expect(Option.isNone(rich_link_metadata_url("javascript:alert(1)"))).toBe(true);
		expect(anchor).not.toContain("fetch(");
		expect(anchor).not.toContain("/favicon.ico");
		expect(anchor).not.toContain("source_url");
	});
});
