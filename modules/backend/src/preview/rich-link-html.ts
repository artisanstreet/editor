import { parse, type DefaultTreeAdapterTypes } from "parse5";

interface RichLinkIconCandidate {
	readonly href: string;
	readonly source: "apple_touch" | "document_icon";
}

const max_metadata_characters = 512;

/** Contains normalized text and URL candidates extracted from parsed HTML. */
export interface ParsedRichLinkHtml {
	readonly base_href: string | undefined;
	readonly icon_candidates: ReadonlyArray<RichLinkIconCandidate>;
	readonly page_name: string | undefined;
	readonly site_name: string | undefined;
	readonly title: string | undefined;
}

function normalize_text(value: string | undefined) {
	const normalized = value?.replace(/\s+/g, " ").trim();

	return normalized && normalized.length > 0
		? [...normalized].slice(0, max_metadata_characters).join("")
		: undefined;
}

function is_element(node: DefaultTreeAdapterTypes.Node): node is DefaultTreeAdapterTypes.Element {
	return "tagName" in node;
}

function attribute(element: DefaultTreeAdapterTypes.Element, name: string) {
	return element.attrs.find((candidate) => candidate.name === name)?.value;
}

function text_content(node: DefaultTreeAdapterTypes.Node): string {
	if ("value" in node && node.nodeName === "#text") {
		return node.value;
	}

	if (!("childNodes" in node)) {
		return "";
	}

	return node.childNodes.map(text_content).join("");
}

/** Parses metadata with parse5 and returns text-only normalized values. */
export function parse_rich_link_html(html: string): ParsedRichLinkHtml {
	const document = parse(html);
	const meta = new Map<string, string>();
	const document_icons: Array<RichLinkIconCandidate> = [];
	const apple_icons: Array<RichLinkIconCandidate> = [];
	let base_href: string | undefined;
	let title: string | undefined;

	const visit = (node: DefaultTreeAdapterTypes.Node) => {
		if (is_element(node)) {
			if (node.tagName === "title" && title === undefined) {
				title = normalize_text(text_content(node));
			}

			if (node.tagName === "base" && base_href === undefined) {
				base_href = normalize_text(attribute(node, "href"));
			}

			if (node.tagName === "meta") {
				const key = normalize_text(
					attribute(node, "property") ?? attribute(node, "name"),
				)?.toLowerCase();
				const content = normalize_text(attribute(node, "content"));

				if (key && content && !meta.has(key)) {
					meta.set(key, content);
				}
			}

			if (node.tagName === "link") {
				const href = normalize_text(attribute(node, "href"));
				const relations = new Set(
					(attribute(node, "rel") ?? "").toLowerCase().split(/\s+/).filter(Boolean),
				);

				if (href && relations.has("icon")) {
					document_icons.push({ href, source: "document_icon" });
				} else if (
					href &&
					(relations.has("apple-touch-icon") ||
						relations.has("apple-touch-icon-precomposed"))
				) {
					apple_icons.push({ href, source: "apple_touch" });
				}
			}
		}

		if ("childNodes" in node) {
			for (const child of node.childNodes) {
				visit(child);
			}
		}
	};

	visit(document);

	return {
		base_href,
		icon_candidates: [...document_icons, ...apple_icons],
		page_name: meta.get("og:title") ?? meta.get("twitter:title") ?? title,
		site_name: meta.get("og:site_name") ?? meta.get("application-name"),
		title,
	};
}
