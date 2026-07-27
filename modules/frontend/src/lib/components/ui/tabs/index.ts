import Root from "./tabs.sv";
import Content from "./tabs-content.sv";
import List, { tabsListVariants, type TabsListVariant } from "./tabs-list.sv";
import Trigger from "./tabs-trigger.sv";

export {
	Root,
	Content,
	List,
	Trigger,
	tabsListVariants,
	type TabsListVariant,
	//
	Root as Tabs,
	Content as TabsContent,
	List as TabsList,
	Trigger as TabsTrigger,
};
