//! Project order and navigation for the project pickers and the command
//! menu.
//!
//! The order starts from the Forge's navigation record (most recently used
//! first) and then follows the user's own navigation, which is reported
//! back to the Forge (see `preferences.rs`).

use super::*;

#[derive(Default)]
pub(super) struct ProjectNavigation {
    loaded_order: bool,
    /// The project order of the Forge's record read on connect; `None`
    /// until it arrives (or when it could not be read).
    pub(super) forge_order: Option<Vec<ProjectId>>,
    /// The route that record resumes.
    pub(super) route: Option<artisan_domain::NavigationRoute>,
    /// The navigation last reported to the Forge.
    pub(super) reported: Option<(ProjectId, Option<ThreadId>)>,
    pub(super) last_threads: HashMap<ProjectId, ThreadId>,
    pub(super) restore_draft: bool,
    pub(super) awaiting_threads: bool,
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
            self.project_navigation
                .forge_order
                .clone()
                .unwrap_or_default()
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
        // The Forge applies the same most-recently-used rule to the report.
        self.report_navigation(project.clone(), None);
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
}
