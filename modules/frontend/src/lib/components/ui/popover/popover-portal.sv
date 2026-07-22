<script lang="ts">
	import { Popover as PopoverPrimitive } from "bits-ui";

	let { ...rest_props }: PopoverPrimitive.PortalProps = $props();
</script>

<PopoverPrimitive.Portal {...rest_props} />
