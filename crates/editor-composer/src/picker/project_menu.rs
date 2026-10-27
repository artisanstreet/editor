//! The project menu (`home_project_picker`).

use super::*;
use editor_ui::project_mark::project_mark;
use editor_ui::scroll_edge::{Edge, scroll_edge};

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
        cx: &Context<Self>,
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
                let visible: Vec<_> = range
                    .clone()
                    .filter_map(|index| this.picker.projects.get(index).cloned())
                    .collect();
                host.update(cx, |host, cx| {
                    for project in &visible {
                        host.load_repository(project, false, cx);
                        host.load_project_icon(project, cx);
                    }
                });
                let chosen_project = this.project(cx);
                let host = host.read(cx);
                range
                    .filter_map(|index| {
                        let project = host.projects.get(this.picker.projects.get(index)?)?;
                        let chosen = chosen_project.as_ref() == Some(&project.id);
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
                                .child(project_mark(
                                    host.project_icon(&project.id),
                                    host.repository(&project.id)
                                        .map(|repository| repository.host.clone()),
                                    16.0,
                                    desktop.text_muted.to_paint(),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(TypographyTokens::size(TextStep::Sm))
                                        .text_color(desktop.text.to_paint())
                                        .child(project.name.clone()),
                                )
                                .when(chosen, |row| {
                                    row.child(
                                        div()
                                            .text_size(TypographyTokens::size(TextStep::Base))
                                            .text_color(desktop.text_muted.to_paint())
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
        .track_scroll(&self.picker.project_scroll)
        .h(px(f32::from(u8::try_from(count.min(8)).unwrap_or(8)) * 36.0));
        let scratch = ProjectId::scratch();
        let offers_scratch = self.host(cx).is_some_and(|host| {
            host.read(cx)
                .projects
                .get(&scratch)
                .is_some_and(|project| !project.removed)
        });
        let mut list = div().flex().flex_col().p(px(8.0));
        if count > 0 {
            let above = self.picker.project_scroll.0.borrow().base_handle.clone();
            let below = above.clone();
            list = list
                .child(
                    div()
                        .relative()
                        .overflow_hidden()
                        .child(project_rows)
                        .child(
                            scroll_edge(
                                Edge::Top,
                                theme.colors.gray.step(7),
                                &self.picker.project_edges[0],
                                move || -f32::from(above.offset().y),
                            )
                            .top_0(),
                        )
                        .child(
                            scroll_edge(
                                Edge::Bottom,
                                theme.colors.gray.step(4),
                                &self.picker.project_edges[1],
                                move || f32::from(below.max_offset().y + below.offset().y),
                            )
                            .bottom_0(),
                        ),
                )
                .child(editor_ui::glass::glass_menu_separator(&theme));
        }
        // The distinct intake rows after a hairline: a thread without a
        // project, and adding a project.
        if offers_scratch {
            let chosen = self.project(cx).as_ref() == Some(&scratch);
            list = list.child(
                self.intake_row(
                    NO_PROJECT_ROW,
                    "No project",
                    AssetId::TABLER_MESSAGE_CIRCLE,
                    chosen,
                    &desktop,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_project(scratch.clone(), cx);
                })),
            );
        }
        list = list.child(
            self.intake_row(
                NEW_PROJECT_ROW,
                "New project",
                AssetId::TABLER_FOLDER_PLUS,
                false,
                &desktop,
                cx,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_menu(cx);
                cx.emit(crate::ComposerEvent::NewProject);
            })),
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

    /// A project menu row that is not a project: its glyph, its label and
    /// the shared hover pill; the caller adds the click.
    fn intake_row(
        &self,
        hover_id: &'static str,
        label: &'static str,
        glyph: AssetId,
        chosen: bool,
        desktop: &ColorTokens,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(SharedString::from(format!("composer-project-{hover_id}")))
            .role(gpui::Role::MenuItem)
            .aria_label(label)
            .aria_selected(chosen)
            .focusable()
            .tab_index(0)
            .debug_selector(move || format!("composer-project-{hover_id}"))
            .relative()
            .flex()
            .items_center()
            .w_full()
            .gap(px(10.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(RadiusTokens::value(RadiusStep::Sm))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.picker.project_hover.set_active(hover_id.to_owned());
                    cx.notify();
                }
            }))
            .child(self.picker.project_hover.row_probe(hover_id.to_owned()))
            .child(
                asset_glyph(glyph)
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(desktop.text_muted.to_paint()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .text_color(desktop.text.to_paint())
                    .child(label),
            )
            .when(chosen, |row| {
                row.child(
                    div()
                        .text_size(TypographyTokens::size(TextStep::Base))
                        .text_color(desktop.text_muted.to_paint())
                        .child("✓"),
                )
            })
    }
}
