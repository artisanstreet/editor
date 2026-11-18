use super::RootViews;
use crate::shell::host::Shell;
use aqua::UiApp;

impl RootViews {
    pub fn arm_qualification_paint(&mut self, shell: &Shell, generation: u64) -> bool {
        let scope = shell.entry.workspace().and_then(|workspace| {
            crate::conversation::presentation::PresentationScope::capture(
                &workspace.conversation,
                &shell.core,
            )
        });
        self.qualification_paint.arm(
            scope,
            generation,
            self.observations.qualification_paint_serial(),
        )
    }

    pub fn qualification_paint(
        &self,
        shell: &Shell,
    ) -> Option<crate::qualification::observation::Paint> {
        let workspace = shell.entry.workspace()?;
        let scope = crate::conversation::presentation::PresentationScope::capture(
            &workspace.conversation,
            &shell.core,
        )?;
        self.qualification_paint.current(&scope)
    }

    pub fn qualification_observation(
        &self,
        shell: &Shell,
        ui: &UiApp,
    ) -> Option<crate::qualification::observation::Observation> {
        let account = shell.entry.qualification_account()?.to_owned();
        let workspace = shell.entry.workspace()?;
        let conversation = &workspace.conversation;
        let scope = crate::conversation::presentation::PresentationScope::capture(
            conversation,
            &shell.core,
        )?;
        let view = conversation.view.as_ref();
        let markdown = workspace.markdown.borrow();
        let prepared_inline_prose = conversation.items().filter_map(|item| {
            let artisan_api::ItemKind::AssistantMessage {
                text: artisan_api::TextContent::Inline(source), ..
            } = &item.kind else { return None; };
            let key = crate::conversation::presentation::BodyKey {
                item: item.id.clone(),
                slot: crate::conversation::fold::Slot::Prose,
            };
            conversation.rows.iter().any(|(row, _)| {
                matches!(row.semantic(), crate::conversation::fold::Row::Text { id, slot: crate::conversation::fold::Slot::Prose, block, .. }
                    if id == &item.id && markdown.prepared_body_current(&scope, &key, source, item.seq, *block))
            }).then(|| item.id.to_string())
        }).collect();
        let bounds = |selector| {
            ui.find_debug_selector(selector)
                .and_then(|node| ui.tree().visual_rect(node))
                .map(crate::qualification::observation::rect_bits)
        };
        Some(crate::qualification::observation::Observation {
            account,
            host: scope.host.0,
            thread: scope.thread.to_string(),
            agent: scope.agent.as_ref().map(ToString::to_string),
            items: conversation.items().cloned().collect(),
            page_loaded: view.is_some_and(|view| view.viewport.loaded()),
            page_in_flight: view.is_some_and(|view| view.viewport.in_flight),
            page_exhausted: view.is_some_and(|view| view.viewport.exhausted),
            page_oldest: view.and_then(|view| view.viewport.oldest),
            page_gap: view.and_then(|view| view.viewport.gap),
            load_error: conversation.load_error.clone(),
            landing: conversation.landing.is_some(),
            work_in_flight: conversation.work_in_flight(),
            row_count: conversation.rows.len(),
            measured_row_count: conversation.row_heights.len(),
            prepared_inline_prose,
            visible_row_start: self
                .transcript_viewport
                .map(|viewport| viewport.visible.start),
            scroll_y_bits: self
                .transcript_viewport
                .map(|viewport| viewport.scroll_y.to_bits()),
            transcript_bounds_bits: bounds("conversation-transcript"),
            composer_bounds_bits: bounds("composer"),
        })
    }
}
