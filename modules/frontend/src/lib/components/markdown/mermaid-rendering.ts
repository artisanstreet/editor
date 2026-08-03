import { renderMermaidSVG } from "beautiful-mermaid";
import { parseDocument } from "htmlparser2";

const MAX_MERMAID_SOURCE_LENGTH = 32_768;
const GOOGLE_FONT_IMPORT = "@import url('https://fonts.googleapis.com/";
const ROOT_STYLE =
	"--bg:var(--background);--fg:var(--foreground);--line:var(--muted-foreground);--accent:var(--foreground);--muted:var(--muted-foreground);--surface:var(--surface-100);--border:var(--border)";
const SAFE_TAGS = new Set([
	"circle",
	"defs",
	"ellipse",
	"g",
	"line",
	"marker",
	"path",
	"polygon",
	"polyline",
	"rect",
	"style",
	"svg",
	"text",
	"title",
	"tspan",
]);
const SAFE_ATTRIBUTES = new Set([
	"class",
	"cx",
	"cy",
	"d",
	"dy",
	"fill",
	"font-size",
	"font-style",
	"font-weight",
	"height",
	"id",
	"marker-end",
	"marker-start",
	"markerheight",
	"markerwidth",
	"orient",
	"points",
	"r",
	"refx",
	"refy",
	"rx",
	"ry",
	"stroke",
	"stroke-dasharray",
	"stroke-linejoin",
	"stroke-width",
	"style",
	"text-anchor",
	"transform",
	"viewbox",
	"width",
	"x",
	"x1",
	"x2",
	"xmlns",
	"y",
	"y1",
	"y2",
]);
const SAFE_PAINT =
	/^(?:#[0-9a-f]{3,8}|black|blue|cyan|gray|green|grey|magenta|none|orange|pink|purple|red|transparent|var\(--(?:_[a-z0-9-]+|bg)\)|white|yellow)$/iu;
const SAFE_LOCAL_REFERENCE = /^url\(#[a-z][a-z0-9_.:-]*\)$/iu;
const ACTIVE_CSS = /@import|expression\s*\(|javascript:|(?:https?:)?\/\/|data:|url\s*\(/iu;

export type MermaidRenderResult =
	| { readonly html: string; readonly status: "rendered" }
	| { readonly status: "invalid" };

const has_safe_attributes = (tag: string, attributes: Readonly<Record<string, string>>) =>
	Object.entries(attributes).every(([raw_name, value]) => {
		const name = raw_name.toLowerCase();
		if (/^on/iu.test(name) || name === "href" || name === "xlink:href") return false;
		if (!SAFE_ATTRIBUTES.has(name) && !/^data-[a-z0-9-]+$/u.test(name)) return false;
		if ((name === "fill" || name === "stroke") && !SAFE_PAINT.test(value)) return false;
		if (
			(name === "marker-start" || name === "marker-end") &&
			!SAFE_LOCAL_REFERENCE.test(value)
		) {
			return false;
		}
		if (name === "stroke-width" && !/^\d+(?:\.\d+)?$/u.test(value)) return false;
		if (name === "stroke-dasharray" && !/^[\d ,.]+$/u.test(value)) return false;
		if (name === "style" && (tag !== "svg" || value !== ROOT_STYLE)) return false;
		if (name === "xmlns" && value !== "http://www.w3.org/2000/svg") return false;
		return true;
	});

/** Structurally validates the trusted renderer's SVG before it reaches `{@html}`. */
export const is_safe_conversation_mermaid_svg = (html: string): boolean => {
	const svg_document = parseDocument(html, { xmlMode: true });
	let svg_roots = 0;

	const visit = (node: (typeof svg_document.children)[number], parent_tag?: string): boolean => {
		if (node.type === "text") {
			return parent_tag !== "style" || !ACTIVE_CSS.test(node.data);
		}
		if (node.type === "comment" || node.type === "cdata" || node.type === "directive") {
			return false;
		}
		if (node.type === "tag") {
			const tag = node.name.toLowerCase();
			if (!SAFE_TAGS.has(tag) || !has_safe_attributes(tag, node.attribs)) return false;
			if (tag === "svg") svg_roots += 1;
			return node.children.every((child) => visit(child, tag));
		}
		return node.children.every((child) => visit(child, parent_tag));
	};

	return svg_document.children.every((node) => visit(node)) && svg_roots === 1;
};

/**
 * Renders Mermaid through beautiful-mermaid's DOM-free escaping renderer, then
 * accepts only a passive SVG subset with local marker references and fixed CSS.
 */
export const render_conversation_mermaid = (source: string): MermaidRenderResult => {
	if (source.length > MAX_MERMAID_SOURCE_LENGTH) return { status: "invalid" };

	try {
		const html = renderMermaidSVG(source, {
			accent: "var(--foreground)",
			bg: "var(--background)",
			border: "var(--border)",
			fg: "var(--foreground)",
			font: "Artisan Neo",
			line: "var(--muted-foreground)",
			muted: "var(--muted-foreground)",
			padding: 24,
			surface: "var(--surface-100)",
			transparent: true,
		})
			.split("\n")
			.filter((line) => !line.includes(GOOGLE_FONT_IMPORT))
			.join("\n");

		return is_safe_conversation_mermaid_svg(html)
			? { html, status: "rendered" }
			: { status: "invalid" };
	} catch {
		return { status: "invalid" };
	}
};
