import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Option } from "effect";
import { describe, expect, it } from "vitest";

import { rich_link_metadata_url } from "../../modules/frontend/src/lib/components/markdown/link-url";

const ReadSource = (path: string) => readFileSync(resolve(path), "utf8");

describe("conversation link presentation", () => {
	it("renders blue non-underlined links with a retained favicon asset", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.sv");
		const links = ReadSource("modules/frontend/src/lib/styles/prose/links.css");

		expect(anchor).toContain('<script lang="ts" effect>');
		expect(anchor).toContain("client.ResolveRichLink({ url })");
		expect(anchor).toContain("client.OpenAsset(favicon.asset_id)");
		expect(anchor).toContain("CreateBrowserObjectUrl(asset.bytes, asset.content_type)");
		expect(anchor).toContain("ReleaseBrowserObjectUrl(source)");
		expect(anchor).toContain("Effect.addFinalizer");
		expect(anchor).toContain("generation !== favicon_generation");
		expect(anchor).toContain("Effect.uninterruptible");
		expect(anchor).toContain('class="conversation-link-favicon"');
		expect(anchor).toContain('alt=""');
		expect(links).toContain("a.conversation-link");
		expect(links).toContain("text-blue-500 no-underline");
		expect(links).toContain("dark:text-blue-400");
	});

	it("resolves favicon metadata only for absolute HTTP(S) destinations", () => {
		const anchor = ReadSource("modules/frontend/src/lib/components/markdown/anchor.sv");

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
