import katex from "katex";

const MAX_MATH_SOURCE_LENGTH = 16_384;

export type MathRenderResult =
	| { readonly html: string; readonly status: "rendered" }
	| { readonly status: "invalid" };

/** Renders untrusted LaTeX with KaTeX's trust boundary and expansion limits. */
export const render_conversation_math = (
	source: string,
	display_mode: boolean,
): MathRenderResult => {
	if (source.length > MAX_MATH_SOURCE_LENGTH) return { status: "invalid" };

	try {
		return {
			html: katex.renderToString(source, {
				displayMode: display_mode,
				maxExpand: 1_000,
				maxSize: 20,
				output: "htmlAndMathml",
				strict: "warn",
				throwOnError: false,
				trust: false,
			}),
			status: "rendered",
		};
	} catch {
		return { status: "invalid" };
	}
};
