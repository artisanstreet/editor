//! Project order and navigation for the sidebar workspace selector.

use super::*;
use crate::home_project_picker::PROJECT_CONTROL_RADIUS_PX;

pub(super) struct ProjectNavigation {
    loaded_order: bool,
    pub(super) last_threads: HashMap<ProjectId, ThreadId>,
    pub(super) restore_draft: bool,
    pub(super) awaiting_threads: bool,
    previous_focus: FocusHandle,
    next_focus: FocusHandle,
}

impl ProjectNavigation {
    pub(super) fn new(cx: &mut Context<NativeApplication>) -> Self {
        Self {
            loaded_order: false,
            last_threads: HashMap::new(),
            restore_draft: false,
            awaiting_threads: false,
            previous_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            next_focus: cx.focus_handle().tab_index(0).tab_stop(true),
        }
    }
}

impl NativeApplication {
    pub(super) fn remembered_project_thread(
        &self,
        project: &ProjectId,
        listing: &ThreadListing,
    ) -> Option<ThreadId> {
        self.project_navigation
            .last_threads
            .get(project)
            .filter(|id| {
                listing
                    .threads()
                    .iter()
                    .any(|thread| &thread.thread_id == *id)
            })
            .cloned()
            .or_else(|| {
                listing
                    .threads()
                    .first()
                    .map(|thread| thread.thread_id.clone())
            })
    }

    pub(super) fn ordered_project_options(
        &mut self,
        listing: &ProjectListing,
    ) -> Vec<ProjectOption> {
        let order = if self.project_navigation.loaded_order || !self.project_options.is_empty() {
            self.project_options
                .iter()
                .map(|project| project.id.clone())
                .collect()
        } else {
            self.project_navigation.loaded_order = true;
            crate::native_last_used::load_project_order(self.machine_home.as_deref())
        };
        self.project_navigation.loaded_order = true;
        let mut options = project_options_from_listing(listing);
        // Stable sorting leaves projects without a saved position in catalog order.
        options.sort_by_key(|project| {
            order
                .iter()
                .position(|id| id == &project.id)
                .unwrap_or(usize::MAX)
        });
        self.project_navigation
            .last_threads
            .retain(|project, _| options.iter().any(|option| &option.id == project));
        options
    }

    pub(super) fn promote_project(&mut self, project: &ProjectId) {
        let Some(index) = self
            .project_options
            .iter()
            .position(|option| &option.id == project)
        else {
            return;
        };
        let option = self.project_options.remove(index);
        self.project_options.insert(0, option);
        // Tests exercise navigation without writing the user's preferences.
        #[cfg(not(test))]
        crate::native_last_used::save_project_order(
            self.machine_home.as_deref(),
            &self
                .project_options
                .iter()
                .map(|option| option.id.clone())
                .collect::<Vec<_>>(),
        );
    }

    pub(super) fn cycle_project(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.project_options.len();
        if count < 2 || !self.project_picker_action_is_admissible() {
            return;
        }
        let current = self
            .project_options
            .iter()
            .position(|option| Some(&option.id) == self.selected_project.as_ref())
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.select_project(self.project_options[next].id.clone(), false, cx);
    }

    pub(super) fn sync_project_pickers(&mut self, cx: &mut Context<Self>) {
        self.install_picker(
            self.project_options.clone(),
            self.selected_project.clone(),
            cx,
        );
        self.install_home_picker(
            self.project_options.clone(),
            self.selected_project.clone(),
            cx,
        );
        self.sync_command_menu_groups(cx);
    }

    pub(super) fn install_sidebar_project_picker(
        &mut self,
        options: Vec<ProjectOption>,
        current: Option<ProjectId>,
        cx: &mut Context<Self>,
    ) {
        let picker =
            cx.new(|cx| HomeProjectPickerView::new(options, current, self.desktop_theme, cx));
        let observation = cx.observe(&picker, |app, picker, cx| {
            app.route_home_picker_action(&picker, cx)
        });
        self.sidebar_project_picker = Some(picker);
        self.sidebar_project_picker_subscription = Some(observation);
    }

    pub(super) fn desktop_project_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let picker = self.sidebar_project_picker.clone().map(|picker| {
            picker.update(cx, |picker, cx| picker.render_sidebar_trigger(window, cx))
        });
        div()
            .id("sidebar-project-switcher")
            .w_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(div().flex_1().min_w(px(0.0)).children(picker))
            .child(self.project_cycle_button(false, cx))
            .child(self.project_cycle_button(true, cx))
    }

    fn project_cycle_button(&self, forward: bool, cx: &Context<Self>) -> Stateful<Div> {
        let enabled = self.project_options.len() > 1 && self.project_picker_action_is_admissible();
        let focus = if forward {
            &self.project_navigation.next_focus
        } else {
            &self.project_navigation.previous_focus
        };
        focus.clone().tab_stop(enabled);
        let id = if forward {
            "sidebar-next-project"
        } else {
            "sidebar-previous-project"
        };
        let button = div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .track_focus(focus)
            .tab_index(0)
            .tab_stop(enabled)
            .w(px(30.0))
            .h(px(30.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .rounded(px(PROJECT_CONTROL_RADIUS_PX))
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&self.theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(
                GlassStrength::Strong,
                px(PROJECT_CONTROL_RADIUS_PX),
            ))
            .child(glass_highlight_layer(
                GlassStrength::Strong,
                px(PROJECT_CONTROL_RADIUS_PX),
            ))
            .text_color(self.desktop_theme.secondary)
            .child(
                asset_glyph(if forward {
                    AssetId::TABLER_CHEVRON_RIGHT
                } else {
                    AssetId::TABLER_CHEVRON_LEFT
                })
                .size(px(14.0)),
            );
        if !enabled {
            return button.opacity(0.35);
        }
        let hover = artisan_ui::gradient::hover_fill_gradient(self.theme);
        button
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |app, _, _, cx| app.cycle_project(forward, cx)))
    }
}
