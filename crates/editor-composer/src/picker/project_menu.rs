//! The project menu (`home_project_picker`).

use super::*;

impl Composer {
    /// The project menu (`home_project_picker`): one row per project, with
    /// the shared hover pill flying between them.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the list, its rows, their probes and the hover pill in visual order"
    )]
    pub(super) fn project_menu(
        &self,
        theme: &ArtisanTheme,
        desktop: &ColorTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (theme, desktop) = (*theme, *desktop);
        let width = PROJECT_MENU_WIDTH_PX.min(self.viewport_width - MENU_VIEWPORT_INSET_X_PX);
        let count = self.picker.projects.len();
        let project_rows = uniform_list(
            "composer-project-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(host) = this.host(cx) else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let project = host
                            .read(cx)
                            .projects
                            .get(this.picker.projects.get(index)?)?;
                        let chosen = this.project().as_ref() == Some(&project.id);
                        let id = project.id.clone();
                        let hover_id = id.to_string();
                        Some(
                            div()
                                .id(SharedString::from(format!("composer-project-{id}")))
                                .role(gpui::Role::MenuItem)
                                .aria_label(project.name.clone())
                                .aria_selected(chosen)
                                .focusable()
                                .tab_index(0)
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("composer-project-row-{id}")
                                })
                                .relative()
                                .flex()
                                .items_center()
                                .w_full()
                                .gap(px(10.0))
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(RadiusTokens::value(RadiusStep::Sm))
                                .cursor_pointer()
                                .on_hover(cx.listener({
                                    let hover_id = hover_id.clone();
                                    move |this, hovered: &bool, _, cx| {
                                        if *hovered {
                                            this.picker.project_hover.set_active(hover_id.clone());
                                            cx.notify();
                                        }
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_project(id.clone(), cx);
                                }))
                                .child(this.picker.project_hover.row_probe(hover_id))
                                .child(
                                    asset_glyph(AssetId::TABLER_FOLDER)
                                        .size(px(16.0))
                                        .text_color(desktop.workspace_secondary.to_paint()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(TypographyTokens::size(TextStep::Sm))
                                        .text_color(desktop.workspace_foreground.to_paint())
                                        .child(project.name.clone()),
                                )
                                .when(chosen, |row| {
                                    row.child(
                                        div()
                                            .text_size(TypographyTokens::size(TextStep::Base))
                                            .text_color(desktop.workspace_secondary.to_paint())
                                            .child("✓"),
                                    )
                                })
                                .h(px(36.0))
                                .into_any_element(),
                        )
                    })
                    .collect()
            }),
        )
        .h(px((count.clamp(1, 8) * 36) as f32));
        let mut list = div().flex().flex_col().p(px(8.0)).child(project_rows);
        // The distinct intake row after a hairline: adding a project.
        list = list.child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_shrink_0()
                        .h(px(1.0))
                        .w_full()
                        .bg(desktop.popover_line.to_paint())
                        .my(px(4.0))
                        .mx(px(4.0)),
                )
                .child(
                    div()
                        .id("composer-project-new")
                        .role(gpui::Role::MenuItem)
                        .aria_label("New project")
                        .focusable()
                        .tab_index(0)
                        .debug_selector(|| "composer-project-new".to_owned())
                        .relative()
                        .flex()
                        .items_center()
                        .w_full()
                        .gap(px(10.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(RadiusTokens::value(RadiusStep::Sm))
                        .cursor_pointer()
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.picker
                                    .project_hover
                                    .set_active(NEW_PROJECT_ROW.to_owned());
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_menu(cx);
                            cx.emit(crate::ComposerEvent::NewProject);
                        }))
                        .child(
                            self.picker
                                .project_hover
                                .row_probe(NEW_PROJECT_ROW.to_owned()),
                        )
                        .child(
                            div()
                                .w(px(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .text_size(TypographyTokens::size(TextStep::Base))
                                .text_color(desktop.workspace_secondary.to_paint())
                                .child("+"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(TypographyTokens::size(TextStep::Sm))
                                .text_color(desktop.workspace_foreground.to_paint())
                                .child("New project"),
                        ),
                ),
        );
        glass_menu(
            "composer-project-menu",
            &theme,
            RadiusTokens::value(RadiusStep::Lg),
        )
        .w(px(width))
        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !*hovered {
                this.picker.project_hover.hide();
                cx.notify();
            }
        }))
        .child(self.picker.project_hover.surface_probe())
        .child(self.picker.project_hover.pill(
            "composer-project-hover",
            &theme,
            RadiusTokens::value(RadiusStep::Sm),
            cx.reduce_motion(),
        ))
        .child(list)
        .into_any_element()
    }
}
