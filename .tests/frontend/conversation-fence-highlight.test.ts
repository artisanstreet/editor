import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	AreConversationLanguagesResident,
	HasPendingConversationFenceTokenization,
	MakeConversationFenceHighlightPlugin,
	RegisterConversationLanguages,
	TokenizeConversationFences,
	TokenizePendingConversationFences,
} from "../../modules/frontend/src/lib/components/markdown/settled-highlighting";

type FencePlugin = ReturnType<typeof MakeConversationFenceHighlightPlugin>;
type PostState = Parameters<NonNullable<FencePlugin["post"]>>[0];
type TreeNodes = PostState["tree"]["nodes"];
type TreeNode = TreeNodes[number];

const typescript_fence = (body: string): TreeNode =>
	["pre", { language: "typescript" }, ["code", {}, body]] as unknown as TreeNode;

const make_state = (nodes: ReadonlyArray<TreeNode>, reusable: ReadonlyArray<TreeNode> = []) =>
	({
		markdown: "",
		options: {},
		reusableNodes: [...reusable],
		tokens: [],
		tree: { frontmatter: {}, meta: {}, nodes: [...nodes] },
	}) as unknown as PostState;

const element = (node: TreeNode): ReadonlyArray<unknown> => node as ReadonlyArray<unknown>;

const run_post = (plugin: FencePlugin, state: PostState) => {
	const post = plugin.post;
	if (!post) throw new Error("fence highlight plugin requires a post hook");
	post(state);
};

describe("conversation fence highlighting", () => {
	it("substitutes cached tokens, skips the open fence, and preserves reused prefixes", async () => {
		await Effect.runPromise(RegisterConversationLanguages(["typescript"]));
		expect(AreConversationLanguagesResident(["typescript"])).toBe(true);
		await Effect.runPromise(
			TokenizeConversationFences([
				{ body: "const reused = 1", language: "typescript" },
				{ body: "const closed = 2", language: "typescript" },
			]),
		);

		const open_body = "const open = tr";
		const plugin = MakeConversationFenceHighlightPlugin(() => open_body);
		const reused = typescript_fence("const reused = 1");
		const closed = typescript_fence("const closed = 2");
		const open = typescript_fence(open_body);
		const prose: TreeNode = ["p", {}, "prose"] as unknown as TreeNode;
		const state = make_state([reused, prose, closed, open], [reused]);
		run_post(plugin, state);

		/** The reused prefix keeps identity so the renderer can skip it. */
		expect(state.tree.nodes[0]).toBe(reused);
		expect(state.tree.nodes[1]).toBe(prose);
		/** The closed fence gains shiki spans and theme classes. */
		const highlighted = element(state.tree.nodes[2] as TreeNode);
		expect(state.tree.nodes[2]).not.toBe(closed);
		expect(String((highlighted[1] as { class?: string }).class)).toContain("shiki");
		const highlighted_code = element(highlighted[2] as TreeNode);
		expect(Array.isArray(highlighted_code[2])).toBe(true);
		expect(JSON.stringify(highlighted)).toContain("closed");
		/** The unterminated fence must stay plain — it grows with every word. */
		expect(state.tree.nodes[3]).toBe(open);
	});

	it("records uncached fences as pending and substitutes after tokenization", async () => {
		await Effect.runPromise(RegisterConversationLanguages(["typescript"]));
		const plugin = MakeConversationFenceHighlightPlugin();
		const body = `const pending = ${Date.now()}`;

		const first_state = make_state([typescript_fence(body)]);
		run_post(plugin, first_state);
		/** The synchronous parse renders plain and owes a tokenization pass. */
		expect(element(first_state.tree.nodes[0] as TreeNode)[0]).toBe("pre");
		expect(
			typeof element(element(first_state.tree.nodes[0] as TreeNode)[2] as TreeNode)[2],
		).toBe("string");
		expect(HasPendingConversationFenceTokenization()).toBe(true);

		await Effect.runPromise(TokenizePendingConversationFences);
		expect(HasPendingConversationFenceTokenization()).toBe(false);

		const second_state = make_state([typescript_fence(body)]);
		run_post(plugin, second_state);
		const highlighted = element(second_state.tree.nodes[0] as TreeNode);
		expect(String((highlighted[1] as { class?: string }).class)).toContain("shiki");
	});

	it("reuses cached tokens for an identical fence body across parses", async () => {
		await Effect.runPromise(RegisterConversationLanguages(["typescript"]));
		const plugin = MakeConversationFenceHighlightPlugin();
		const body = "const cached = true";
		await Effect.runPromise(TokenizeConversationFences([{ body, language: "typescript" }]));

		const first_state = make_state([typescript_fence(body)]);
		run_post(plugin, first_state);
		const second_state = make_state([typescript_fence(body)]);
		run_post(plugin, second_state);

		const first_code = element(element(first_state.tree.nodes[0] as TreeNode)[2] as TreeNode);
		const second_code = element(element(second_state.tree.nodes[0] as TreeNode)[2] as TreeNode);
		expect(first_code.length).toBeGreaterThan(2);
		/** The token nodes themselves are shared, not re-tokenised. */
		expect(second_code[2]).toBe(first_code[2]);
	});

	it("highlights fences nested below the top level", async () => {
		await Effect.runPromise(RegisterConversationLanguages(["typescript"]));
		const plugin = MakeConversationFenceHighlightPlugin();
		const body = "const nested = 3";
		await Effect.runPromise(TokenizeConversationFences([{ body, language: "typescript" }]));

		const nested = typescript_fence(body);
		const quote: TreeNode = ["blockquote", {}, nested] as unknown as TreeNode;
		const state = make_state([quote]);
		run_post(plugin, state);

		const rewritten_quote = element(state.tree.nodes[0] as TreeNode);
		expect(state.tree.nodes[0]).not.toBe(quote);
		const rewritten_pre = element(rewritten_quote[2] as TreeNode);
		expect(String((rewritten_pre[1] as { class?: string }).class)).toContain("shiki");
	});
});
