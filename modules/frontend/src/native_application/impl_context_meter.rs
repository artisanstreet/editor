//! The titlebar context-window gauge and its hover card.
//!
//! Hovering the gauge pops the details card in below it, and leaving pops it
//! back out, on the profile menu's own motion: the same open and close
//! durations, scaling out of and back into the corner it is anchored to.

use super::*;

impl NativeApplication {
    /// The meter for the open thread's validated context reading, wrapped so
    /// hovering it opens the details card; `None` off a thread or without a
    /// reading.
    pub(super) fn desktop_context_meter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let presentation = self.context_presentation(cx)?;
        let bounds = Rc::clone(&self.context_meter_bounds);
        // The composer's ring gauge is the trigger; hovering it opens the
        // card with the progress bar.
        let meter = crate::native_context_usage::render_context_ring(
            &presentation,
            ArtisanTheme::for_mode(ThemeMode::Dark),
        )
        .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
            app.set_context_card_open(*hovered, cx);
        }));
        Some(
            div()
                .child(meter)
                .on_children_prepainted(move |children, _, _| {
                    if let Some(first) = children.first() {
                        bounds.set(*first);
                    }
                })
                .into_any_element(),
        )
    }

    /// The details card below the meter while it is open or closing.
    pub(super) fn desktop_context_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let phase = self.context_card_motion.borrow().phase();
        if !self.context_card_open && phase != PickerMenuPhase::Closing {
            return None;
        }
        let presentation = self.context_presentation(cx)?;
        let card = crate::native_context_usage::render_context_card(
            &presentation,
            ArtisanTheme::for_mode(ThemeMode::Dark),
        );
        let meter = self.context_meter_bounds.get();
        let motion = *self.context_card_motion.borrow();
        Some(
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::TopRight)
                    .position(meter.top_right())
                    .offset(gpui::point(px(0.0), meter.size.height + px(8.0)))
                    .child(animate_picker_menu_timed(
                        card,
                        Rc::clone(&self.context_card_motion),
                        motion,
                        "context",
                        // The card hangs below the meter, aligned to its
                        // right edge: it grows from that top-right corner.
                        PickerMenuEntrance::Scale {
                            origin: gpui::point(1.0, 0.0),
                        },
                        PROFILE_MENU_OPEN,
                        PROFILE_MENU_CLOSE,
                    )),
            )
            .into_any_element(),
        )
    }

    fn context_presentation(
        &self,
        cx: &App,
    ) -> Option<crate::native_context_usage::NativeContextUsagePresentation> {
        if !matches!(self.route(), NativeRoute::Thread { .. }) {
            return None;
        }
        let snapshot = self.composer_controls.read(cx).snapshot();
        // The thread's latest reading shows whichever run is live: a new
        // run keeps the previous reading until it reports its own.
        snapshot
            .context_usage
            .as_ref()
            .and_then(|usage| usage.presentation(Some(usage.reporting_run_id.as_str())))
    }

    fn set_context_card_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open == self.context_card_open {
            return;
        }
        self.context_card_open = open;
        let generation = if open {
            self.context_card_motion.borrow_mut().begin_open()
        } else {
            self.context_card_motion.borrow_mut().begin_close()
        };
        if cx.reduce_motion() {
            let mut motion = self.context_card_motion.borrow_mut();
            if open {
                motion.finish_open(generation);
            } else {
                motion.finish_close(generation);
            }
            self.context_card_task = None;
        } else {
            let duration = if open {
                PROFILE_MENU_OPEN
            } else {
                PROFILE_MENU_CLOSE
            };
            self.context_card_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |app, cx| {
                    let settled = {
                        let mut motion = app.context_card_motion.borrow_mut();
                        if open {
                            motion.finish_open(generation)
                        } else {
                            motion.finish_close(generation)
                        }
                    };
                    if settled {
                        app.context_card_task = None;
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }
}
