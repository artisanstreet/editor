use crate::sidebar::list::{HostSnapshot, RowState, listed, row_state};
use artisan_api::{Project, ProjectId, ThreadId};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadRow {
    pub id: ThreadId,
    pub title: String,
    pub state: RowState,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    pub project: ProjectId,
    pub name: String,
    pub threads: Vec<ThreadRow>,
}

#[must_use]
pub fn title_of(title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("New thread")
        .to_owned()
}

#[must_use]
pub fn groups<'a>(
    host: &impl HostSnapshot,
    projects: impl IntoIterator<Item = &'a Project>,
    open: Option<&ThreadId>,
) -> Vec<Group> {
    let mut threads = host.threads();
    threads.retain(|thread| listed(thread));
    threads.sort_by(|a, b| {
        b.activity_seq
            .cmp(&a.activity_seq)
            .then_with(|| b.id.0.cmp(&a.id.0))
    });
    let mut idle = projects
        .into_iter()
        .filter(|project| !project.removed)
        .collect::<Vec<_>>();
    idle.sort_by_key(|a| a.name.to_lowercase());
    let mut groups: Vec<Group> = Vec::new();
    for thread in threads {
        let Some(project) = idle.iter().find(|project| project.id == thread.project_id) else {
            continue;
        };
        let row = ThreadRow {
            id: thread.id.clone(),
            title: title_of(thread.title.as_deref()),
            state: row_state(host, thread),
            selected: open == Some(&thread.id),
        };
        match groups.iter_mut().find(|group| group.project == thread.project_id) {
            Some(group) => group.threads.push(row),
            None => groups.push(Group {
                project: project.id.clone(),
                name: project.name.clone(),
                threads: vec![row],
            }),
        }
    }
    let shown = groups
        .iter()
        .map(|group| group.project.clone())
        .collect::<HashSet<_>>();
    groups.extend(
        idle.into_iter()
            .filter(|project| !shown.contains(&project.id))
            .map(|project| Group {
                project: project.id.clone(),
                name: project.name.clone(),
                threads: Vec::new(),
            }),
    );
    groups
}

pub const ROW_HEIGHT: f32 = 32.0;
pub const ROW_GAP: f32 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub project: ProjectId,
    pub top: f32,
    pub end: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub row: f32,
    pub gap: f32,
}

fn snapped(logical: f32, scale: f32) -> f32 {
    let device = logical * scale;
    (device.abs() - 0.5).ceil().copysign(device) / scale
}

impl Metrics {
    #[must_use]
    pub fn at(scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        Self {
            row: snapped(ROW_HEIGHT, scale),
            gap: snapped(ROW_GAP, scale),
        }
    }

    #[must_use]
    pub fn body_height(self, threads: usize) -> f32 {
        match threads {
            0 => 0.0,
            count => self.gap + count as f32 * self.row + (count - 1) as f32 * self.gap,
        }
    }
}

#[must_use]
pub fn sections(groups: &[Group], metrics: Metrics, openness: impl Fn(&ProjectId) -> f32) -> Vec<Section> {
    let mut top = 0.0;
    groups
        .iter()
        .map(|group| {
            let section = Section {
                project: group.project.clone(),
                top,
                end: top + metrics.row + openness(&group.project) * metrics.body_height(group.threads.len()),
            };
            top = section.end + metrics.gap;
            section
        })
        .collect()
}

#[must_use]
pub fn pinned(sections: &[Section], metrics: Metrics, offset: f32) -> Option<(ProjectId, f32)> {
    sections
        .iter()
        .find(|section| section.top < offset && offset < section.end && section.end - section.top > metrics.row)
        .map(|section| {
            (
                section.project.clone(),
                (section.end - offset - metrics.row).min(0.0),
            )
        })
}
