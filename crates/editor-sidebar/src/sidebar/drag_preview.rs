use aqua_core::{NodeId, Point};
use aqua_ui::{
    element::Element,
    input::DragSnapshot,
    overlay::{Align, DismissPolicy, OverlayId, OverlayKind, Side},
    popup::PopupOptions,
};

use editor_core::{DraggedThread, HostInstance};
use editor_ui::theme::ArtisanTheme;

pub fn preview_position(snapshot: &DragSnapshot) -> Point {
    Point::new(
        snapshot.pointer.position.x - snapshot.cursor_offset.x,
        snapshot.pointer.position.y - snapshot.cursor_offset.y,
    )
}

pub fn preview(title: &str, theme: ArtisanTheme) -> Element {
    super::row::drag_preview(title, theme).exclude_from_hit_testing()
}

pub struct DragPreviewHost {
    overlay: OverlayId,
    source: NodeId,
    instance: HostInstance,
    payload: DraggedThread,
    position: Point,
}

impl DragPreviewHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: NodeId,
        instance: HostInstance,
        snapshot: &DragSnapshot,
        theme: ArtisanTheme,
    ) -> Result<Option<Self>, String> {
        let Some(payload) = snapshot.payload.get::<DraggedThread>().cloned() else {
            return Ok(None);
        };
        let position = preview_position(snapshot);
        let mut options = PopupOptions::tooltip().at(position);
        options.kind = OverlayKind::Popover;
        options.dismiss = DismissPolicy::MANUAL;
        options.placement.preferred = Side::Below;
        options.placement.align = Align::Start;
        options.placement.gap = 0.0;
        options.placement.flip = false;
        let content = preview(&payload.title, theme);
        let overlay = app
            .open_popup_measured(owner, content, options)
            .map_err(|error| error.to_string())?;
        if app.set_popup_uncontained(overlay, true).is_none() {
            app.close_popup(overlay);
            return Err("drag preview could not be positioned".to_owned());
        }
        Ok(Some(Self {
            overlay,
            source: snapshot.source,
            instance,
            payload,
            position,
        }))
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<NodeId> {
        app.host().popup_root(self.overlay)
    }

    pub fn sync(
        &mut self,
        app: &mut aqua::UiApp,
        instance: Option<HostInstance>,
        snapshot: Option<&DragSnapshot>,
    ) -> Result<bool, String> {
        let Some(snapshot) = snapshot.filter(|snapshot| {
            instance == Some(self.instance)
                && snapshot.source == self.source
                && snapshot.payload.get::<DraggedThread>() == Some(&self.payload)
        }) else {
            app.close_popup(self.overlay);
            return Ok(false);
        };
        if !app.host().is_open(self.overlay) {
            return Ok(false);
        }
        let position = preview_position(snapshot);
        if position != self.position {
            app.set_popup_pointer_anchor(self.overlay, position)
                .ok_or_else(|| "drag preview could not be moved".to_owned())?;
            self.position = position;
        }
        Ok(true)
    }

    pub fn dispose(self, app: &mut aqua::UiApp) {
        app.close_popup(self.overlay);
    }
}
