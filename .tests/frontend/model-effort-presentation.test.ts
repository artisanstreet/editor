import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { thinking_level_labels } from "../../modules/frontend/src/lib/engine/model-selection";

const ReadSelectorSource = (file: string) =>
	readFileSync(
		resolve(
			import.meta.dirname,
			"../../modules/frontend/src/routes/components/model-selector",
			file,
		),
		"utf8",
	);

const ReadPolicyControls = () => ReadSelectorSource("policy-controls.svelte");
const ReadOptionTooltip = () => ReadSelectorSource("option-tooltip.svelte");
const ReadSelectorView = () => ReadSelectorSource("view.svelte");
const ReadPresentation = () =>
	readFileSync(
		resolve(import.meta.dirname, "../../modules/frontend/src/lib/engine/presentation.ts"),
		"utf8",
	);

describe("model effort presentation", () => {
	it("renders catalog-owned base and special groups with a conditional separator", () => {
		const source = ReadPolicyControls();

		expect(source).toContain('option.presentation_group === "base"');
		expect(source).toContain('option.presentation_group === "special"');
		expect(source).toContain("<SelectGroupHeading");
		expect(source).toContain("Efforts");
		expect(source).toContain("Special Efforts");
		expect(source).toContain("{#if special_thinking_options.length > 0}");
		expect(source).toContain("<SelectSeparator");
		expect(source).toContain("data-[orientation=horizontal]:w-auto");
		expect(source).not.toMatch(/option\.id === ["'](?:max|ultra)["']/u);
	});

	it("uses the shared option tooltip only when an effort has catalog caveats", () => {
		const source = ReadPolicyControls();

		expect(source).toContain("{#if option.description !== undefined}");
		expect(source).toContain("<OptionTooltip");
		expect(source).toContain("description={option.description}");
		expect(source).toContain("advisory={option.advisory}");
	});

	/**
	 * Every row in the picker describes itself the same way. Four hand-rolled
	 * copies of the same trigger-and-content pair is how they drifted apart in
	 * the first place, so the component is the single place the treatment lives.
	 */
	it("routes every picker tooltip through the one component", () => {
		const source = ReadPolicyControls();

		expect(source).not.toContain("<TooltipContent");
		expect(source).not.toContain("<TooltipTrigger");
		expect(source.match(/<OptionTooltip/gu)).toHaveLength(4);
	});

	it("wears the dropdown's own surface and warns in the destructive tone", () => {
		const tooltip = ReadOptionTooltip();

		expect(tooltip).toContain("<ShaderGlassSurface");
		expect(tooltip).toContain('strength="strong"');
		/** A caret can only be painted in a solid fill, which glass has none of. */
		expect(tooltip).toContain("arrow={false}");
		expect(tooltip).toContain("bg-transparent!");
		expect(tooltip).toContain("text-destructive");
	});

	it("labels every special canonical level without exposing native vocabulary", () => {
		expect(thinking_level_labels.max).toBe("Max");
		expect(thinking_level_labels.ultra).toBe("Ultra");
	});
});

describe("model preview configuration", () => {
	it("gives the OpenAI Codex route the canonical OpenAI catalog mark", () => {
		const presentation = ReadPresentation();

		expect(presentation).toContain(
			'openai: { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true }',
		);
		expect(presentation).toContain(
			'"openai-codex": { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true }',
		);
	});

	it("maps OpenCode model labs to their own provider marks", () => {
		const presentation = ReadPresentation();

		expect(presentation).toContain(
			'nvidia: { accent: "#76b900", icon: NvidiaLogo, monochrome: false }',
		);
		expect(presentation).toContain(
			'meta: { accent: "#0081fb", icon: SvglMetaLogo, monochrome: false }',
		);
		expect(presentation).toContain(
			'tencent: { accent: "#006cb6", icon: TencentLogo, monochrome: false }',
		);
	});

	it("renders live routes as shared collapsible groups with provider labs", () => {
		const view = ReadSelectorView();
		const list = ReadSelectorSource("model-list.svelte");

		expect(view).toContain(
			"RouteGroupsForModels(effective_catalog, active_engine, active_models)",
		);
		expect(view).toContain("{route_groups}");
		expect(view).not.toContain("unavailable_route_groups");
		expect(list).toContain("route_groups: ReadonlyArray<ModelRouteGroup>");
		expect(list).toContain("{#each route_groups as group (group.id)}");
		expect(list).toContain("<Collapsible open");
		expect(list).toContain("<CollapsibleTrigger");
		expect(list).toContain("<ChevronRight");
		expect(list).toContain("ProviderMarkFor(model.definition.provider)");
		expect(list).toContain("{model.lab}");
		expect(list).toContain('group.unavailable_reason ?? "No models available"');
	});

	it("keeps reported context out of the policy selector stack", () => {
		const controls = ReadPolicyControls();
		const summary = ReadSelectorSource("model-preview-summary.svelte");
		const view = ReadSelectorView();

		expect(controls).not.toContain("formatted_context_tokens");
		expect(controls).not.toContain("context_window_tokens");
		expect(view).toContain("<ModelPreviewSummary model={previewed_model} />");
		expect(summary).toContain("FormatContextWindowTokens");
		expect(summary).toContain("context_window");
		expect(summary).toContain("{context_window}");
	});

	/**
	 * The panel used to clear the preview the moment the pointer reached it, so
	 * the settings on screen were always the already-selected model's and a
	 * hovered model could not be configured at all without being clicked first.
	 */
	it("keeps the previewed model while the pointer moves onto its settings", () => {
		const view = ReadSelectorView();

		expect(view).toContain("model={previewed_model}");
		expect(view).not.toContain("onpointerenter={yield* ResetPreview}");
		/** Closing the picker is the one thing that still forgets the preview. */
		expect(view).toContain(
			"if (!open && previewed_model_id !== undefined) yield* ResetPreview;",
		);
	});

	it("offers the previewed model's own harness permissions, not the selection's", () => {
		const view = ReadSelectorView();

		expect(view).toContain("permission_options={previewed_permissions?.options ?? []}");
		expect(view).toContain("permission_default={previewed_permissions?.default}");
	});

	it("shows the current session permission for models that are only previewed", () => {
		const controls = ReadPolicyControls();

		expect(controls).toContain(
			"permission_options.find((option) => option.id === permission_mode)",
		);
		expect(controls).not.toMatch(
			/model\.id === selected_model_id\s*\? permission_options\.find/u,
		);
	});

	it("presents provider permissions on one canonical scale", () => {
		const controls = ReadPolicyControls();
		const compaction = readFileSync(
			resolve(
				process.cwd(),
				"modules/frontend/src/routes/components/settings/compaction-model.svelte",
			),
			"utf8",
		);

		expect(controls).toContain("PermissionLevelLabel(current_permission)");
		expect(controls).toContain("PermissionLevelLabel(option)");
		expect(compaction).toContain("label: PermissionLevelLabel(option)");
	});

	/**
	 * Hovering still adopts nothing; touching a control does. A setting is a
	 * statement about the model it sits under, so there is nothing for it to
	 * mean until that model is the one the thread will use.
	 */
	it("adopts the previewed model when a setting is touched, and only then", () => {
		const view = ReadSelectorView();

		expect(view).toContain("const AdoptForConfiguration = (model: ModelChoice)");
		expect(view).toContain("if (selected_model?.id === model.id) return true;");
		expect(
			view.match(/if \(!\(yield\* AdoptForConfiguration\(model\)\)\) return;/gu),
		).toHaveLength(4);
		expect(view).not.toContain("if (selected_model?.id !== model.id) return;");
		expect(view).toContain("const PreviewModel = (model_id: string)");
	});
});
