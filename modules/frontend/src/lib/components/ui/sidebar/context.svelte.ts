import { IsMobile } from "$lib/hooks/is-mobile.svelte.ts";
import { play } from "cuelume";
import { getContext, setContext } from "svelte";
import { sidebar_keyboard_shortcut } from "$lib/components/ui/sidebar/constants.ts";

type Getter<T> = () => T;

export type SidebarMotionPhase = "idle" | "children-exiting";

/**
 * Props used to create shared sidebar state.
 */
export type SidebarStateProps = {
	mobile_breakpoint?: Getter<number | undefined>;

	/**
	 * A getter function that returns the current open state of the sidebar.
	 * We use a getter function here to support `bind:open` on the `Sidebar.Provider`
	 * component.
	 */
	open: Getter<boolean>;

	/**
	 * A function that sets the open state of the sidebar. To support `bind:open`, we need
	 * a source of truth for changing the open state to ensure it will be synced throughout
	 * the sub-components and any `bind:` references.
	 */
	set_open: (open: boolean) => void;
};

class SidebarState {
	readonly props: SidebarStateProps;
	motion_phase = $state<SidebarMotionPhase>("idle");
	open = $derived.by(() => this.props.open());
	open_mobile = $state(false);
	set_open: SidebarStateProps["set_open"];
	#is_mobile: IsMobile;
	state = $derived.by(() => (this.open ? "expanded" : "collapsed"));

	constructor(props: SidebarStateProps) {
		this.set_open = props.set_open;
		this.#is_mobile = new IsMobile(props.mobile_breakpoint?.());
		this.props = props;
	}

	/**
	 * Whether the current viewport matches the mobile sidebar breakpoint.
	 */
	get is_mobile() {
		return this.#is_mobile.current;
	}

	/**
	 * Event handler to apply to the `<svelte:window>`.
	 */
	handle_shortcut_keydown = (event: KeyboardEvent) => {
		if (event.key === sidebar_keyboard_shortcut && (event.metaKey || event.ctrlKey)) {
			event.preventDefault();
			this.toggle();
		}
	};

	set_open_mobile = (value: boolean) => {
		this.open_mobile = value;
	};

	set_motion_phase = (phase: SidebarMotionPhase) => {
		this.motion_phase = phase;
	};

	toggle = () => {
		play("toggle");

		return this.#is_mobile.current
			? (this.open_mobile = !this.open_mobile)
			: this.set_open(!this.open);
	};
}

const symbol_key = "scn-sidebar";

/**
 * Instantiates a new `SidebarState` instance and sets it in the context.
 */
export function set_sidebar(props: SidebarStateProps): SidebarState {
	return setContext(Symbol.for(symbol_key), new SidebarState(props));
}

/**
 * Retrieves the `SidebarState` instance from the context. This is a class instance,
 * so you cannot destructure it.
 */
export function use_sidebar(): SidebarState {
	return getContext(Symbol.for(symbol_key));
}
