use agent_settings::{AgentSettings, THREADS_LIST_MAX_WIDTH, THREADS_LIST_MIN_WIDTH};
use editor::Editor;
use feature_flags::{AgentThreadWorktreeLabelFlag, FeatureFlag};
use gpui::{
    Action as _, AnyElement, App, Context, Decorations, DismissEvent, Entity, FocusHandle,
    Focusable, KeyContext, ListState, Modifiers, Pixels, Render, SharedString, Task, TaskExt,
    WeakEntity, Window, WindowBackgroundAppearance, list, prelude::*, px,
};
use menu::{
    Cancel, Confirm, SelectChild, SelectFirst, SelectLast, SelectNext, SelectParent, SelectPrevious,
};
use platform_title_bar::apply_title_bar_insets;
use project::{Event as ProjectEvent, repo_identity_path_if_local};
use recent_projects::sidebar_recent_projects::SidebarRecentProjects;
use remote::RemoteConnectionOptions;
use ui::utils::platform_title_bar_height;

use serde::{Deserialize, Serialize};
use settings::{Settings as _, SettingsStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use theme::{ActiveTheme, CLIENT_SIDE_DECORATION_ROUNDING};
use ui::{
    CommonAnimationExt, ContextMenu, ContextMenuEntry, GradientFade, HighlightedLabel, KeyBinding,
    PopoverMenu, PopoverMenuHandle, ProjectEmptyState, ScrollAxes, Scrollbars, Tab, TintColor,
    Tooltip, WithScrollbar, prelude::*, render_modifiers,
};
use util::ResultExt as _;
use util::path_list::PathList;
use workspace::{
    CloseWindow, FocusWorkspaceSidebar, MoveProjectDown, MoveProjectUp, MultiWorkspace,
    MultiWorkspaceEvent, NextProject, Open, OpenMode, PreviousProject, ProjectGroupKey,
    RemovalIntent, Sidebar as WorkspaceSidebar, SidebarSide, ToggleWorkspaceSidebar, Workspace,
    sidebar_side_context_menu,
};

use zed_actions::OpenRecent;
use zed_actions::editor::{MoveDown, MoveUp};

#[cfg(test)]
mod sidebar_tests;

gpui::actions!(
    agents_sidebar,
    [
        /// Creates a new thread in the currently selected or active project group.
        NewThreadInGroup,
        /// Toggles between the thread list and the thread history.
        ToggleThreadHistory,
    ]
);

gpui::actions!(
    dev,
    [
        /// Dumps multi-workspace state (projects, worktrees, active threads) into a new buffer.
        DumpWorkspaceInfo,
    ]
);

#[derive(Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SerializedSidebarView {
    #[default]
    ThreadList,
    #[serde(alias = "Archive")]
    History,
}

#[derive(Default, Serialize, Deserialize)]
struct SerializedSidebar {
    #[serde(default)]
    width: Option<f32>,
    /// Whether `width` came from the user dragging the divider.
    ///
    /// Legacy state recorded every width without this flag. A width other than
    /// the old default of 300 pixels still identifies a manual resize. Only
    /// that ambiguous default falls back to `agent.threads_sidebar.default_width`.
    #[serde(default)]
    width_set_by_user: bool,
    #[serde(default)]
    active_view: SerializedSidebarView,
}

#[derive(Clone)]
enum ListEntry {
    ProjectHeader {
        key: ProjectGroupKey,
        label: SharedString,
        highlight_positions: Vec<usize>,
        has_running_threads: bool,
        waiting_thread_count: usize,
        has_notifications: bool,
        is_active: bool,
        has_threads: bool,
    },
}

#[cfg(test)]
impl ListEntry {
    fn reachable_workspaces<'a>(
        &'a self,
        multi_workspace: &'a workspace::MultiWorkspace,
        cx: &'a App,
    ) -> Vec<Entity<Workspace>> {
        let ListEntry::ProjectHeader { key, .. } = self;
        multi_workspace.workspaces_for_project_group(key, cx)
    }
}

#[derive(Default)]
struct SidebarContents {
    entries: Vec<ListEntry>,
    project_header_indices: Vec<usize>,
    has_open_projects: bool,
}

/// Identity-and-layout key for a [`ListEntry`] used to preserve measured list items
/// across rebuilds. Equal shapes must render to the same height; add any new
/// height-affecting state here.
#[derive(Debug, PartialEq, Eq)]
enum EntryShape {
    ProjectHeader {
        key: ProjectGroupKey,
        // Toggles the "No threads yet" empty-state row when not collapsed.
        has_threads: bool,
        // Determines whether the "No threads yet" row is rendered (only shown when
        // `!is_collapsed && !has_threads`).
        is_collapsed: bool,
    },
}

fn workspace_path_list(workspace: &Entity<Workspace>, cx: &App) -> PathList {
    PathList::new(&workspace.read(cx).root_paths(cx))
}

#[derive(Clone)]
struct WorkspaceMenuWorktreeLabel {
    icon: Option<IconName>,
    primary_name: SharedString,
    secondary_name: Option<SharedString>,
}

impl WorkspaceMenuWorktreeLabel {
    fn render(&self) -> impl IntoElement {
        h_flex()
            .min_w_0()
            .gap_0p5()
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
            })
            .child(Label::new(self.primary_name.clone()).truncate())
            .when_some(self.secondary_name.clone(), |this, secondary_name| {
                this.child(Label::new("/").alpha(0.5))
                    .child(Label::new(secondary_name).truncate())
            })
    }
}

fn workspace_menu_worktree_labels(
    workspace: &Entity<Workspace>,
    cx: &App,
) -> Vec<WorkspaceMenuWorktreeLabel> {
    let root_paths = workspace.read(cx).root_paths(cx);
    let show_folder_name = root_paths.len() > 1;
    let project = workspace.read(cx).project().clone();
    let (path_style, repository_snapshots) = {
        let project = project.read(cx);
        let path_style = project.path_style(cx);
        let repository_snapshots = project
            .repositories(cx)
            .values()
            .map(|repo| repo.read(cx).snapshot())
            .collect::<Vec<_>>();
        (path_style, repository_snapshots)
    };

    root_paths
        .into_iter()
        .map(|root_path| {
            let root_path = root_path.as_ref();
            let folder_name = path_style
                .file_name(root_path)
                .map(|name| SharedString::from(name.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let repository_snapshot = repository_snapshots
                .iter()
                .find(|snapshot| snapshot.work_directory_abs_path.as_ref() == root_path);

            if let Some(snapshot) = repository_snapshot {
                let worktree_name = if snapshot.is_linked_worktree() {
                    let identity_fallback = repo_identity_path_if_local(
                        &snapshot.common_dir_abs_path,
                        snapshot.path_style,
                    );
                    snapshot
                        .main_worktree_abs_path()
                        .or(identity_fallback)
                        .and_then(|name_anchor_path| {
                            project::linked_worktree_short_name(
                                name_anchor_path,
                                root_path,
                                snapshot.path_style,
                            )
                        })
                        .unwrap_or_else(|| folder_name.clone())
                } else {
                    "main".into()
                };

                if show_folder_name {
                    WorkspaceMenuWorktreeLabel {
                        icon: Some(IconName::GitWorktree),
                        primary_name: folder_name,
                        secondary_name: Some(worktree_name),
                    }
                } else {
                    WorkspaceMenuWorktreeLabel {
                        icon: Some(IconName::GitWorktree),
                        primary_name: worktree_name,
                        secondary_name: None,
                    }
                }
            } else {
                WorkspaceMenuWorktreeLabel {
                    icon: None,
                    primary_name: folder_name,
                    secondary_name: None,
                }
            }
        })
        .collect()
}

/// Shows a [`RemoteConnectionModal`] on the given workspace and establishes
/// an SSH connection. Suitable for passing to
/// [`MultiWorkspace::find_or_create_workspace`] as the `connect_remote`
/// argument.
fn connect_remote(
    modal_workspace: Entity<Workspace>,
    connection_options: RemoteConnectionOptions,
    window: &mut Window,
    cx: &mut Context<MultiWorkspace>,
) -> gpui::Task<anyhow::Result<Option<Entity<remote::RemoteClient>>>> {
    remote_connection::connect_with_modal(&modal_workspace, connection_options, window, cx)
}

/// The sidebar re-derives its entire entry list from scratch on every
/// change via `update_entries` → `rebuild_contents`. Avoid adding
/// incremental or inter-event coordination state — if something can
/// be computed from the current world state, compute it in the rebuild.
pub struct Sidebar {
    multi_workspace: WeakEntity<MultiWorkspace>,
    width: Pixels,
    /// Whether `width` came from the user rather than from
    /// `agent.threads_sidebar.default_width`. Only a user-chosen width is persisted, so
    /// that changing the setting is not overridden by a width the user never
    /// picked. Serialization runs on many triggers besides resizing.
    width_set_by_user: bool,
    focus_handle: FocusHandle,
    filter_editor: Entity<Editor>,
    list_state: ListState,
    contents: SidebarContents,
    /// The index of the list item that currently has the keyboard focus
    ///
    /// Note: This is NOT the same as the active item.
    selection: Option<usize>,

    recent_projects_popover_handle: PopoverMenuHandle<SidebarRecentProjects>,
    project_header_menu_handles: HashMap<usize, PopoverMenuHandle<ContextMenu>>,
    project_header_new_thread_menu_handles: HashMap<usize, PopoverMenuHandle<ContextMenu>>,
    project_header_menu_ix: Option<usize>,
    _subscriptions: Vec<gpui::Subscription>,
    _draft_editor_observations: Vec<gpui::Subscription>,
    update_task: Option<Task<()>>,
}

impl Sidebar {
    pub fn new(
        multi_workspace: Entity<MultiWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        AgentThreadWorktreeLabelFlag::watch(cx);

        let mut previous_default_width =
            AgentSettings::get_global(cx).threads_sidebar.default_width;
        cx.observe_global::<SettingsStore>(move |this, cx| {
            let width = AgentSettings::get_global(cx).threads_sidebar.default_width;
            if previous_default_width != width {
                previous_default_width = width;
                this.set_width(None, cx);
                this.serialize(cx);
            }
        })
        .detach();

        let filter_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Search threads…", window, cx);
            editor
        });

        cx.subscribe_in(
            &multi_workspace,
            window,
            |this, _multi_workspace, event: &MultiWorkspaceEvent, window, cx| match event {
                MultiWorkspaceEvent::ActiveWorkspaceChanged { .. } => {
                    this.schedule_update_entries(false, cx);
                }
                MultiWorkspaceEvent::WorkspaceAdded(workspace) => {
                    this.subscribe_to_workspace(workspace, window, cx);
                    this.schedule_update_entries(false, cx);
                }
                MultiWorkspaceEvent::WorkspaceRemoved(_)
                | MultiWorkspaceEvent::ProjectGroupsChanged => {
                    this.schedule_update_entries(false, cx);
                }
            },
        )
        .detach();

        cx.subscribe(&filter_editor, |this: &mut Self, _, event, cx| {
            if let editor::EditorEvent::BufferEdited = event {
                let query = this.filter_editor.read(cx).text(cx);
                if !query.is_empty() {
                    this.selection.take();
                }
                this.schedule_update_entries(!query.is_empty(), cx);
            }
        })
        .detach();

        let deferred_multi_workspace = multi_workspace.downgrade();
        cx.defer_in(window, move |this, window, cx| {
            if let Some(multi_workspace) = deferred_multi_workspace.upgrade() {
                let workspaces: Vec<_> = multi_workspace.read(cx).workspaces().cloned().collect();
                for workspace in &workspaces {
                    this.subscribe_to_workspace(workspace, window, cx);
                }
            }
            this.schedule_update_entries(false, cx);
        });

        Self {
            multi_workspace: multi_workspace.downgrade(),
            width: AgentSettings::get_global(cx).threads_sidebar.default_width,
            width_set_by_user: false,
            focus_handle,
            filter_editor,
            list_state: ListState::new(0, gpui::ListAlignment::Top, px(1000.)),
            contents: SidebarContents::default(),
            selection: None,
            recent_projects_popover_handle: PopoverMenuHandle::default(),
            project_header_menu_handles: HashMap::new(),
            project_header_new_thread_menu_handles: HashMap::new(),
            project_header_menu_ix: None,
            _subscriptions: Vec::new(),
            _draft_editor_observations: Vec::new(),
            update_task: None,
        }
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        cx.emit(workspace::SidebarEvent::SerializeNeeded);
    }

    fn is_group_collapsed(&self, key: &ProjectGroupKey, cx: &App) -> bool {
        self.multi_workspace
            .upgrade()
            .and_then(|mw| {
                mw.read(cx)
                    .group_state_by_key(key)
                    .map(|state| !state.expanded)
            })
            .unwrap_or(false)
    }

    fn set_group_expanded(&self, key: &ProjectGroupKey, expanded: bool, cx: &mut Context<Self>) {
        if let Some(mw) = self.multi_workspace.upgrade() {
            mw.update(cx, |mw, cx| {
                if let Some(state) = mw.group_state_by_key_mut(key) {
                    state.expanded = expanded;
                }
                mw.serialize(cx);
            });
        }
    }

    fn is_active_workspace(&self, workspace: &Entity<Workspace>, cx: &App) -> bool {
        self.multi_workspace
            .upgrade()
            .map_or(false, |mw| mw.read(cx).workspace() == workspace)
    }

    fn subscribe_to_workspace(
        &mut self,
        workspace: &Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = workspace.read(cx).project().clone();
        if project.read(cx).is_via_collab() {
            return;
        }

        cx.subscribe_in(
            &project,
            window,
            |this, _project, event, _window, cx| match event {
                ProjectEvent::WorktreeAdded(_)
                | ProjectEvent::WorktreeRemoved(_)
                | ProjectEvent::WorktreeOrderChanged => {
                    this.schedule_update_entries(false, cx);
                }
                ProjectEvent::WorktreePathsChanged { .. } => {
                    this.schedule_update_entries(false, cx);
                }
                _ => {}
            },
        )
        .detach();

        let git_store = workspace.read(cx).project().read(cx).git_store().clone();
        cx.subscribe_in(
            &git_store,
            window,
            |this, _, event: &project::git_store::GitStoreEvent, _window, cx| {
                if matches!(
                    event,
                    project::git_store::GitStoreEvent::RepositoryUpdated(
                        _,
                        project::git_store::RepositoryEvent::GitWorktreeListChanged
                            | project::git_store::RepositoryEvent::HeadChanged,
                        _,
                    )
                ) {
                    this.schedule_update_entries(false, cx);
                }
            },
        )
        .detach();

        self.observe_docks(workspace, cx);
    }

    fn observe_docks(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        let docks: Vec<_> = workspace
            .read(cx)
            .all_docks()
            .into_iter()
            .cloned()
            .collect();
        let workspace = workspace.downgrade();
        for dock in docks {
            let workspace = workspace.clone();
            cx.observe(&dock, move |this, _dock, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                if !this.is_active_workspace(&workspace, cx) {
                    return;
                }

                cx.notify();
            })
            .detach();
        }
    }

    /// Opens a new workspace for a group that has no open workspaces.
    fn open_workspace_for_group(
        &mut self,
        project_group_key: &ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let path_list = project_group_key.path_list().clone();
        let host = project_group_key.host();
        let provisional_key = Some(project_group_key.clone());
        let active_workspace = multi_workspace.read(cx).workspace().clone();
        let modal_workspace = active_workspace.clone();

        let task = multi_workspace.update(cx, |this, cx| {
            this.find_or_create_workspace(
                path_list,
                host,
                provisional_key,
                |options, window, cx| connect_remote(active_workspace, options, window, cx),
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        });

        cx.spawn_in(window, async move |_this, cx| {
            let result = task.await;
            remote_connection::dismiss_connection_modal(&modal_workspace, cx);
            result?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// Rebuilds the sidebar contents from current workspace and thread state.
    ///
    /// Iterates [`MultiWorkspace::project_group_keys`] to determine project
    /// groups, then populates thread entries from the metadata store and
    /// merges live thread info from active agent panels.
    ///
    /// Aim for a single forward pass over workspaces and threads plus an
    /// O(T log T) sort. Avoid adding extra scans over the data.
    ///
    /// Properties:
    ///
    /// - Should always show every workspace in the multiworkspace
    ///     - If you have no threads, and two workspaces for the worktree and the main workspace, make sure at least one is shown
    /// - Should always show every thread, associated with each workspace in the multiworkspace
    /// - After every build_contents, our "active" state should exactly match the current workspace's, current agent panel's current thread.
    fn rebuild_contents(&mut self, cx: &App) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let mw = multi_workspace.read(cx);
        let workspaces: Vec<_> = mw.workspaces().cloned().collect();
        let active_workspace = Some(mw.workspace().clone());

        let mut entries = Vec::new();
        let mut project_header_indices: Vec<usize> = Vec::new();

        let has_open_projects = workspaces
            .iter()
            .any(|ws| !workspace_path_list(ws, cx).paths().is_empty());

        let groups = mw.project_groups(cx);

        let mut all_paths: Vec<PathBuf> = groups
            .iter()
            .flat_map(|group| group.key.path_list().paths().iter().cloned())
            .collect();
        all_paths.sort_unstable();
        all_paths.dedup();
        let path_details =
            util::disambiguate::compute_disambiguation_details(&all_paths, |path, detail| {
                project::path_suffix(path, detail)
            });
        let path_detail_map: HashMap<PathBuf, usize> =
            all_paths.into_iter().zip(path_details).collect();

        for group in &groups {
            let group_key = &group.key;
            let group_workspaces = &group.workspaces;
            if group_key.path_list().paths().is_empty() {
                continue;
            }

            let label = group_key.display_name(&path_detail_map);
            let is_active = active_workspace
                .as_ref()
                .is_some_and(|active| group_workspaces.contains(active));

            project_header_indices.push(entries.len());
            entries.push(ListEntry::ProjectHeader {
                key: group_key.clone(),
                label,
                highlight_positions: Vec::new(),
                has_running_threads: false,
                waiting_thread_count: 0,
                has_notifications: false,
                is_active,
                has_threads: false,
            });
        }

        self.contents = SidebarContents {
            entries,
            project_header_indices,
            has_open_projects,
        };
    }

    fn schedule_update_entries(&mut self, select_first_after_update: bool, cx: &mut Context<Self>) {
        if self.update_task.is_some() && !select_first_after_update {
            return;
        }

        self.update_task = Some(cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| {
                this.update_task = None;
                this.update_entries(cx);
                if select_first_after_update {
                    this.select_first_entry();
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Rebuilds the sidebar's visible entries from already-cached state.
    fn update_entries(&mut self, cx: &mut Context<Self>) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        if !multi_workspace.read(cx).multi_workspace_enabled(cx) {
            return;
        }

        let had_notifications = self.has_notifications(cx);
        let previous_shapes: Vec<EntryShape> =
            self.entry_shapes(multi_workspace.read(cx)).collect();

        self.rebuild_contents(cx);

        // Preserve measurements for unchanged entries so sticky headers do not flicker.
        self.apply_list_state_diff(&previous_shapes, multi_workspace.read(cx));

        if had_notifications != self.has_notifications(cx) {
            multi_workspace.update(cx, |_, cx| {
                cx.notify();
            });
        }

        cx.notify();
    }

    /// Splices only the changed entry range, leaving unchanged item measurements intact.
    fn apply_list_state_diff(
        &self,
        previous_shapes: &[EntryShape],
        multi_workspace: &MultiWorkspace,
    ) {
        let mut new_iter = self.entry_shapes(multi_workspace);
        let mut prefix_len = 0;
        let leading_new = loop {
            match (previous_shapes.get(prefix_len), new_iter.next()) {
                (Some(prev), Some(next)) if *prev == next => prefix_len += 1,
                (None, None) => return,
                (_, leading) => break leading,
            }
        };

        let new_tail: Vec<EntryShape> = leading_new.into_iter().chain(new_iter).collect();
        let prev_tail = &previous_shapes[prefix_len..];
        let suffix_len = prev_tail
            .iter()
            .rev()
            .zip(new_tail.iter().rev())
            .take_while(|(prev, next)| prev == next)
            .count();

        let old_changed = prefix_len..previous_shapes.len() - suffix_len;
        let new_changed_count = new_tail.len() - suffix_len;
        self.list_state.splice(old_changed, new_changed_count);
    }

    fn entry_shapes<'a>(
        &'a self,
        multi_workspace: &'a MultiWorkspace,
    ) -> impl Iterator<Item = EntryShape> + 'a {
        self.contents.entries.iter().map(move |entry| match entry {
            ListEntry::ProjectHeader {
                key, has_threads, ..
            } => EntryShape::ProjectHeader {
                key: key.clone(),
                has_threads: *has_threads,
                is_collapsed: multi_workspace
                    .group_state_by_key(key)
                    .map(|state| !state.expanded)
                    .unwrap_or(false),
            },
        })
    }

    fn select_first_entry(&mut self) {
        self.selection = (!self.contents.entries.is_empty()).then_some(0);
    }

    fn render_list_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.contents.entries.get(ix) else {
            return div().into_any_element();
        };
        let is_focused = self.focus_handle.is_focused(window);
        // is_selected means the keyboard selector is here.
        let is_selected = is_focused && self.selection == Some(ix);

        let is_group_header_after_first =
            ix > 0 && matches!(entry, ListEntry::ProjectHeader { .. });

        let rendered = match entry {
            ListEntry::ProjectHeader {
                key,
                label,
                highlight_positions,
                has_running_threads,
                waiting_thread_count,
                has_notifications,
                is_active: is_active_group,
                has_threads,
            } => {
                self.project_header_menu_handles.entry(ix).or_default();
                self.project_header_new_thread_menu_handles
                    .entry(ix)
                    .or_default();

                self.render_project_header(
                    ix,
                    false,
                    key,
                    label,
                    highlight_positions,
                    *has_running_threads,
                    *waiting_thread_count,
                    *has_notifications,
                    *is_active_group,
                    is_selected,
                    *has_threads,
                    // has_active_draft,
                    cx,
                )
            }
        };

        if is_group_header_after_first {
            v_flex()
                .w_full()
                .border_t_1()
                .border_color(cx.theme().colors().border)
                .child(rendered)
                .into_any_element()
        } else {
            rendered
        }
    }

    fn render_remote_project_icon(
        &self,
        ix: usize,
        host: Option<&RemoteConnectionOptions>,
    ) -> Option<AnyElement> {
        let remote_icon_per_type = match host? {
            RemoteConnectionOptions::Wsl(_) => IconName::Linux,
            RemoteConnectionOptions::Docker(_) => IconName::Box,
            _ => IconName::Server,
        };

        Some(
            div()
                .id(format!("remote-project-icon-{}", ix))
                .child(
                    Icon::new(remote_icon_per_type)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                )
                .tooltip(Tooltip::text("Remote Project"))
                .into_any_element(),
        )
    }

    fn render_project_header(
        &self,
        ix: usize,
        is_sticky: bool,
        key: &ProjectGroupKey,
        label: &SharedString,
        highlight_positions: &[usize],
        has_running_threads: bool,
        waiting_thread_count: usize,
        has_notifications: bool,
        is_active: bool,
        is_focused: bool,
        has_threads: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host = key.host();

        let has_filter = self.has_filter_query(cx);

        let id_prefix = if is_sticky { "sticky-" } else { "" };
        let id = SharedString::from(format!("{id_prefix}project-header-{ix}"));
        let group_name = SharedString::from(format!("{id_prefix}header-group-{ix}"));

        let is_collapsed = self.is_group_collapsed(key, cx);
        let disclosure_icon = if is_collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };

        let _key_for_toggle = key.clone();
        let key_for_focus = key.clone();

        let color = cx.theme().colors();
        let sidebar_base_bg = if is_sticky {
            color.panel_background_for_overlay()
        } else {
            color.panel_background
        };

        // The fade gradient renders as a visible patch on transparent windows,
        // so truncate the label instead.
        let opaque_window = cx.theme().window_background_appearance()
            == WindowBackgroundAppearance::Opaque
            && sidebar_base_bg.a >= 1.0;

        let label = if highlight_positions.is_empty() {
            Label::new(label.clone())
                .when(!is_active, |this| this.color(Color::Muted))
                .when(!opaque_window, |this| this.truncate())
                .into_any_element()
        } else {
            HighlightedLabel::new(label.clone(), highlight_positions.to_vec())
                .when(!is_active, |this| this.color(Color::Muted))
                .when(!opaque_window, |this| this.truncate())
                .into_any_element()
        };

        let base_bg = color.background.blend(sidebar_base_bg);

        let hover_solid = base_bg.blend(color.ghost_element_hover);
        let active_solid = base_bg.blend(color.ghost_element_active);

        let group_name_for_gradient = group_name.clone();
        let gradient_overlay = move || {
            GradientFade::new(base_bg, hover_solid, active_solid)
                .width(px(92.0))
                .right(px(-2.0))
                .gradient_stop(0.7)
                .when(!has_filter, |this| {
                    this.group_name(group_name_for_gradient.clone())
                })
        };

        let header = h_flex()
            .id(id)
            .group(&group_name)
            .when(!has_filter, |this| this.cursor_pointer())
            .relative()
            .h(Tab::content_height(cx))
            .w_full()
            .pl_2()
            .pr_1p5()
            .justify_between()
            .border_1()
            .border_r_2()
            .map(|this| {
                if is_focused {
                    this.border_color(color.panel_focused_border)
                } else {
                    this.border_color(gpui::transparent_black())
                }
            })
            .when(!has_filter, |this| {
                this.hover(|s| s.bg(color.ghost_element_hover))
                    .group_active(&group_name, |s| s.bg(color.ghost_element_active))
            })
            .child(
                h_flex()
                    .relative()
                    .min_w_0()
                    .w_full()
                    .gap_1()
                    .child(label)
                    .when_some(
                        self.render_remote_project_icon(ix, host.as_ref()),
                        |this, icon| this.child(icon),
                    )
                    .when(is_collapsed, |this| {
                        this.when(has_running_threads, |this| {
                            this.child(
                                Icon::new(IconName::LoadCircle)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted)
                                    .with_rotate_animation(2),
                            )
                        })
                        .when(waiting_thread_count > 0, |this| {
                            let tooltip_text = if waiting_thread_count == 1 {
                                "1 thread is waiting for confirmation".to_string()
                            } else {
                                format!(
                                    "{waiting_thread_count} threads are waiting for confirmation",
                                )
                            };
                            this.child(
                                div()
                                    .id(format!("{id_prefix}waiting-indicator-{ix}"))
                                    .child(
                                        Icon::new(IconName::Warning)
                                            .size(IconSize::XSmall)
                                            .color(Color::Warning),
                                    )
                                    .tooltip(Tooltip::text(tooltip_text)),
                            )
                        })
                        .when(
                            has_notifications && !has_running_threads && waiting_thread_count == 0,
                            |this| {
                                this.child(
                                    Icon::new(IconName::Circle)
                                        .size(IconSize::Small)
                                        .color(Color::Accent),
                                )
                            },
                        )
                    })
                    .when(!has_filter, |this| {
                        this.child(
                            div()
                                .when(!is_focused, |this| this.visible_on_hover(&group_name))
                                .child(
                                    Icon::new(disclosure_icon)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    }),
            )
            .children(opaque_window.then(|| gradient_overlay()))
            .child(
                h_flex()
                    .children(opaque_window.then(|| gradient_overlay()))
                    .child(
                        h_flex()
                            .gap_px()
                            .pr_1p5()
                            .child(self.render_project_header_ellipsis_menu(
                                ix,
                                id_prefix,
                                key,
                                is_active,
                                has_threads,
                                &group_name,
                                cx,
                            ))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            }),
                    ),
            )
            .on_mouse_down(gpui::MouseButton::Right, {
                let menu_handle = self
                    .project_header_menu_handles
                    .get(&ix)
                    .cloned()
                    .unwrap_or_default();
                move |_, window, cx| {
                    cx.stop_propagation();
                    menu_handle.toggle(window, cx);
                }
            })
            .on_click(
                cx.listener(move |this, _event: &gpui::ClickEvent, window, cx| {
                    this.activate_or_open_workspace_for_group(&key_for_focus, window, cx);
                }),
            )
            .block_mouse_except_scroll();

        header.into_any_element()
    }

    fn render_project_header_ellipsis_menu(
        &self,
        ix: usize,
        id_prefix: &str,
        project_group_key: &ProjectGroupKey,
        is_active: bool,
        has_threads: bool,
        group_name: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let multi_workspace = self.multi_workspace.clone();
        let project_group_key = project_group_key.clone();

        let show_multi_project_entries = multi_workspace
            .read_with(cx, |mw, _| {
                project_group_key.host().is_none() && mw.project_group_keys().len() >= 2
            })
            .unwrap_or(false);

        let this = cx.weak_entity();

        let trigger_id = SharedString::from(format!("{id_prefix}-ellipsis-menu-{ix}"));
        let menu_handle = self
            .project_header_menu_handles
            .get(&ix)
            .cloned()
            .unwrap_or_default();
        let is_menu_open = menu_handle.is_deployed();

        PopoverMenu::new(format!("{id_prefix}project-header-menu-{ix}"))
            .with_handle(menu_handle)
            .trigger(
                IconButton::new(trigger_id, IconName::Ellipsis)
                    .when(!is_menu_open && !self.has_filter_query(cx), |button| {
                        let color = cx.theme().colors();
                        button
                            .hover_background(color.element_background)
                            .active_background(color.element_active)
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .icon_size(IconSize::Small)
                    .when(!is_menu_open, |el| el.visible_on_hover(group_name)),
            )
            .on_open(Rc::new({
                let this = this.clone();
                move |_window, cx| {
                    this.update(cx, |sidebar, cx| {
                        sidebar.project_header_menu_ix = Some(ix);
                        cx.notify();
                    })
                    .ok();
                }
            }))
            .menu(move |window, cx| {
                let multi_workspace = multi_workspace.clone();
                let project_group_key = project_group_key.clone();
                let this_for_menu = this.clone();

                let open_workspaces = multi_workspace
                    .read_with(cx, |multi_workspace, cx| {
                        multi_workspace.workspaces_for_project_group(&project_group_key, cx)
                    })
                    .unwrap_or_default();

                // Compute reorder state at menu-open time so it reflects the
                // most recent group ordering.
                let (group_index, total_groups) = multi_workspace
                    .read_with(cx, |mw, _| {
                        let keys = mw.project_group_keys();
                        let index = keys.iter().position(|k| k == &project_group_key);
                        (index, keys.len())
                    })
                    .unwrap_or((None, 0));
                let show_reorder_entries = total_groups >= 2;
                let can_move_up = group_index.is_some_and(|i| i > 0);
                let can_move_down = group_index.is_some_and(|i| i + 1 < total_groups);

                let active_workspace = multi_workspace
                    .read_with(cx, |multi_workspace, _cx| {
                        multi_workspace.workspace().clone()
                    })
                    .ok();
                let workspace_labels: Vec<_> = open_workspaces
                    .iter()
                    .map(|workspace| workspace_menu_worktree_labels(workspace, cx))
                    .collect();
                let workspace_is_active: Vec<_> = open_workspaces
                    .iter()
                    .map(|workspace| active_workspace.as_ref() == Some(workspace))
                    .collect();

                let menu =
                    ContextMenu::build_persistent(window, cx, move |menu, _window, menu_cx| {
                        let menu = menu.end_slot_action(Box::new(menu::SecondaryConfirm));
                        let weak_menu = menu_cx.weak_entity();

                        let menu = menu.when(show_multi_project_entries, |this| {
                            this.entry(
                                "Open Project in New Window",
                                Some(Box::new(workspace::MoveProjectToNewWindow)),
                                {
                                    let project_group_key = project_group_key.clone();
                                    let multi_workspace = multi_workspace.clone();
                                    move |window, cx| {
                                        multi_workspace
                                            .update(cx, |multi_workspace, cx| {
                                                multi_workspace
                                                    .open_project_group_in_new_window(
                                                        &project_group_key,
                                                        window,
                                                        cx,
                                                    )
                                                    .detach_and_log_err(cx);
                                            })
                                            .ok();
                                    }
                                },
                            )
                        });

                        let menu = menu
                            .custom_entry(
                                {
                                    move |_window, cx| {
                                        let action = h_flex()
                                            .opacity(0.6)
                                            .children(render_modifiers(
                                                &Modifiers::secondary_key(cx),
                                                PlatformStyle::platform(),
                                                None,
                                                Some(TextSize::Default.rems(cx).into()),
                                                false,
                                            ))
                                            .child(Label::new("-click").color(Color::Muted));

                                        let label = if has_threads {
                                            "Focus Last Project"
                                        } else {
                                            "Focus Project"
                                        };

                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .gap_4()
                                            .child(
                                                Label::new(label)
                                                    .when(is_active, |s| s.color(Color::Disabled)),
                                            )
                                            .child(action)
                                            .into_any_element()
                                    }
                                },
                                {
                                    let project_group_key = project_group_key.clone();
                                    let this = this_for_menu.clone();
                                    move |window, cx| {
                                        if is_active {
                                            return;
                                        }
                                        this.update(cx, |sidebar, cx| {
                                            if let Some(workspace) =
                                                sidebar.workspace_for_group(&project_group_key, cx)
                                            {
                                                sidebar.activate_workspace(&workspace, window, cx);
                                            } else {
                                                sidebar.open_workspace_for_group(
                                                    &project_group_key,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            sidebar.selection = None;
                                        })
                                        .ok();
                                    }
                                },
                            )
                            .selectable(!is_active);

                        let menu = if open_workspaces.is_empty() {
                            menu
                        } else {
                            let mut menu = menu.separator().header("Open Worktrees");

                            for (
                                workspace_index,
                                ((workspace, workspace_label), is_active_workspace),
                            ) in open_workspaces
                                .iter()
                                .cloned()
                                .zip(workspace_labels.iter().cloned())
                                .zip(workspace_is_active.iter().copied())
                                .enumerate()
                            {
                                let activate_multi_workspace = multi_workspace.clone();
                                let close_multi_workspace = multi_workspace.clone();
                                let activate_weak_menu = weak_menu.clone();
                                let close_weak_menu = weak_menu.clone();
                                let activate_workspace = workspace.clone();
                                let close_workspace = workspace.clone();

                                menu = menu.custom_entry(
                                    move |_window, _cx| {
                                        let close_multi_workspace = close_multi_workspace.clone();
                                        let close_weak_menu = close_weak_menu.clone();
                                        let close_workspace = close_workspace.clone();
                                        let row_group_name = SharedString::from(format!(
                                            "workspace-menu-row-{workspace_index}"
                                        ));

                                        h_flex()
                                            .group(&row_group_name)
                                            .w_full()
                                            .gap_2()
                                            .justify_between()
                                            .child(h_flex().min_w_0().gap_1().children(
                                                workspace_label.iter().enumerate().map(
                                                    |(label_ix, label)| {
                                                        h_flex()
                                                            .gap_1()
                                                            .when(label_ix > 0, |this| {
                                                                this.child(
                                                                    Label::new("•").alpha(0.25),
                                                                )
                                                            })
                                                            .child(label.render())
                                                            .into_any_element()
                                                    },
                                                ),
                                            ))
                                            .when(is_active_workspace, |this| {
                                                this.pr_1().child(
                                                    Icon::new(IconName::Check)
                                                        .size(IconSize::Small)
                                                        .color(Color::Accent),
                                                )
                                            })
                                            .when(!is_active_workspace, |this| {
                                                let close_multi_workspace =
                                                    close_multi_workspace.clone();
                                                let close_weak_menu = close_weak_menu.clone();
                                                let close_workspace = close_workspace.clone();

                                                this.child(
                                                    IconButton::new(
                                                        ("close-workspace", workspace_index),
                                                        IconName::Close,
                                                    )
                                                    .icon_size(IconSize::Small)
                                                    .visible_on_hover(&row_group_name)
                                                    .tooltip(Tooltip::text("Close Worktree"))
                                                    .on_click(move |_, window, cx| {
                                                        cx.stop_propagation();
                                                        window.prevent_default();
                                                        close_multi_workspace
                                                            .update(cx, |multi_workspace, cx| {
                                                                multi_workspace
                                                                    .remove(
                                                                        [close_workspace.clone()],
                                                                        RemovalIntent::CloseProject,
                                                                        window,
                                                                        cx,
                                                                    )
                                                                    .detach_and_log_err(cx);
                                                            })
                                                            .ok();
                                                        close_weak_menu
                                                            .update(cx, |_, cx| {
                                                                cx.emit(DismissEvent)
                                                            })
                                                            .ok();
                                                    }),
                                                )
                                            })
                                            .into_any_element()
                                    },
                                    move |window, cx| {
                                        activate_multi_workspace
                                            .update(cx, |multi_workspace, cx| {
                                                multi_workspace.activate(
                                                    activate_workspace.clone(),
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                        activate_weak_menu
                                            .update(cx, |_, cx| cx.emit(DismissEvent))
                                            .ok();
                                    },
                                );
                            }

                            menu
                        };

                        let menu = menu.when(show_reorder_entries, |this| {
                            let move_up_multi_workspace = multi_workspace.clone();
                            let move_up_key = project_group_key.clone();
                            let move_up_weak_menu = weak_menu.clone();
                            let move_down_multi_workspace = multi_workspace.clone();
                            let move_down_key = project_group_key.clone();
                            let move_down_weak_menu = weak_menu.clone();

                            this.separator()
                                .item(
                                    ContextMenuEntry::new("Move Up")
                                        .action(Box::new(MoveProjectUp))
                                        .disabled(!can_move_up)
                                        .handler(move |_window, cx| {
                                            move_up_multi_workspace
                                                .update(cx, |mw, cx| {
                                                    mw.move_project_group_up(&move_up_key, cx);
                                                })
                                                .ok();
                                            move_up_weak_menu
                                                .update(cx, |_, cx| cx.emit(DismissEvent))
                                                .ok();
                                        }),
                                )
                                .item(
                                    ContextMenuEntry::new("Move Down")
                                        .action(Box::new(MoveProjectDown))
                                        .disabled(!can_move_down)
                                        .handler(move |_window, cx| {
                                            move_down_multi_workspace
                                                .update(cx, |mw, cx| {
                                                    mw.move_project_group_down(&move_down_key, cx);
                                                })
                                                .ok();
                                            move_down_weak_menu
                                                .update(cx, |_, cx| cx.emit(DismissEvent))
                                                .ok();
                                        }),
                                )
                        });

                        let project_group_key = project_group_key.clone();
                        let remove_multi_workspace = multi_workspace.clone();
                        menu.separator().entry("Remove", None, move |window, cx| {
                            remove_multi_workspace
                                .update(cx, |multi_workspace, cx| {
                                    multi_workspace
                                        .remove_project_group(&project_group_key, window, cx)
                                        .detach_and_log_err(cx);
                                })
                                .ok();
                            weak_menu.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
                        })
                    });

                let this = this.clone();

                window
                    .subscribe(&menu, cx, move |_, _: &gpui::DismissEvent, _window, cx| {
                        this.update(cx, |sidebar, cx| {
                            sidebar.project_header_menu_ix = None;
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();

                Some(menu)
            })
            .anchor(gpui::Anchor::TopRight)
            .offset(gpui::Point {
                x: px(0.),
                y: px(1.),
            })
            .into_any_element()
    }

    fn render_sticky_header(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let scroll_top = self.list_state.logical_scroll_top();

        let &header_idx = self
            .contents
            .project_header_indices
            .iter()
            .rev()
            .find(|&&idx| idx <= scroll_top.item_ix)?;

        let needs_sticky = header_idx < scroll_top.item_ix
            || (header_idx == scroll_top.item_ix && scroll_top.offset_in_item > px(0.));

        if !needs_sticky {
            return None;
        }

        let ListEntry::ProjectHeader {
            key,
            label,
            highlight_positions,
            has_running_threads,
            waiting_thread_count,
            has_notifications,
            is_active,
            has_threads,
        } = self.contents.entries.get(header_idx)?;

        let is_focused = self.focus_handle.is_focused(window);
        let is_selected = is_focused && self.selection == Some(header_idx);

        let header_element = self.render_project_header(
            header_idx,
            true,
            key,
            &label,
            &highlight_positions,
            *has_running_threads,
            *waiting_thread_count,
            *has_notifications,
            *is_active,
            is_selected,
            *has_threads,
            cx,
        );

        let top_offset = self
            .contents
            .project_header_indices
            .iter()
            .find(|&&idx| idx > header_idx)
            .and_then(|&next_idx| {
                let bounds = self.list_state.bounds_for_item(next_idx)?;
                let viewport = self.list_state.viewport_bounds();
                let y_in_viewport = bounds.origin.y - viewport.origin.y;
                let header_height = bounds.size.height;
                (y_in_viewport < header_height).then_some(y_in_viewport - header_height)
            })
            .unwrap_or(px(0.));

        let color = cx.theme().colors();
        let background = color.panel_background_for_overlay();

        let element = v_flex()
            .absolute()
            .top(top_offset)
            .left_0()
            .w_full()
            .bg(background)
            .border_b_1()
            .border_color(color.border.opacity(0.5))
            .child(header_element)
            .shadow_sm()
            .into_any_element();

        Some(element)
    }

    fn toggle_collapse(
        &mut self,
        project_group_key: &ProjectGroupKey,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_collapsed = self.is_group_collapsed(project_group_key, cx);
        self.set_group_expanded(project_group_key, is_collapsed, cx);
        self.update_entries(cx);
    }

    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults(cx.platform_info().os);
        dispatch_context.add("ThreadsSidebar");
        dispatch_context.add("menu");

        let identifier = if self.filter_editor.focus_handle(cx).is_focused(window) {
            "searching"
        } else {
            "not_searching"
        };

        dispatch_context.add(identifier);
        dispatch_context
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        cx.notify();
    }

    fn has_filter_query(&self, cx: &App) -> bool {
        !self.filter_editor.read(cx).text(cx).is_empty()
    }

    fn editor_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.select_next(&SelectNext, window, cx);
        if self.selection.is_some() {
            self.focus_handle.focus(window, cx);
        }
    }

    fn editor_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.select_previous(&SelectPrevious, window, cx);
        if self.selection.is_some() {
            self.focus_handle.focus(window, cx);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _window: &mut Window, cx: &mut Context<Self>) {
        let next = match self.selection {
            Some(ix) if ix + 1 < self.contents.entries.len() => ix + 1,
            Some(_) if !self.contents.entries.is_empty() => 0,
            None if !self.contents.entries.is_empty() => 0,
            _ => return,
        };
        self.selection = Some(next);
        self.list_state.scroll_to_reveal_item(next);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        match self.selection {
            Some(0) => {
                self.selection = None;
                self.focus_handle.focus(window, cx);
                cx.notify();
            }
            Some(ix) => {
                self.selection = Some(ix - 1);
                self.list_state.scroll_to_reveal_item(ix - 1);
                cx.notify();
            }
            None if !self.contents.entries.is_empty() => {
                let last = self.contents.entries.len() - 1;
                self.selection = Some(last);
                self.list_state.scroll_to_reveal_item(last);
                cx.notify();
            }
            None => {}
        }
    }

    fn select_first(&mut self, _: &SelectFirst, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.contents.entries.is_empty() {
            self.selection = Some(0);
            self.list_state.scroll_to_reveal_item(0);
            cx.notify();
        }
    }

    fn select_last(&mut self, _: &SelectLast, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(last) = self.contents.entries.len().checked_sub(1) {
            self.selection = Some(last);
            self.list_state.scroll_to_reveal_item(last);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selection else { return };
        let Some(entry) = self.contents.entries.get(ix) else {
            return;
        };

        match entry {
            ListEntry::ProjectHeader { key, .. } => {
                let key = key.clone();
                self.toggle_collapse(&key, window, cx);
            }
        }
    }

    fn expand_selected_entry(
        &mut self,
        _: &SelectChild,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selection else { return };

        match self.contents.entries.get(ix) {
            Some(ListEntry::ProjectHeader { key, .. }) => {
                let key = key.clone();
                if self.is_group_collapsed(&key, cx) {
                    self.set_group_expanded(&key, true, cx);
                    self.update_entries(cx);
                } else if ix + 1 < self.contents.entries.len() {
                    self.selection = Some(ix + 1);
                    self.list_state.scroll_to_reveal_item(ix + 1);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn collapse_selected_entry(
        &mut self,
        _: &SelectParent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selection else { return };

        match self.contents.entries.get(ix) {
            Some(ListEntry::ProjectHeader { key, .. }) => {
                let key = key.clone();
                if !self.is_group_collapsed(&key, cx) {
                    self.set_group_expanded(&key, false, cx);
                    self.update_entries(cx);
                }
            }
            None => {}
        }
    }

    fn toggle_selected_fold(
        &mut self,
        _: &editor::actions::ToggleFold,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.selection else { return };

        // Find the group header for the current selection.
        let header_ix = match self.contents.entries.get(ix) {
            Some(ListEntry::ProjectHeader { .. }) => Some(ix),
            None => None,
        };

        if let Some(header_ix) = header_ix {
            if let Some(ListEntry::ProjectHeader { key, .. }) = self.contents.entries.get(header_ix)
            {
                let key = key.clone();
                if self.is_group_collapsed(&key, cx) {
                    self.set_group_expanded(&key, true, cx);
                } else {
                    self.selection = Some(header_ix);
                    self.set_group_expanded(&key, false, cx);
                }
                self.update_entries(cx);
            }
        }
    }

    fn fold_all(
        &mut self,
        _: &editor::actions::FoldAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mw) = self.multi_workspace.upgrade() {
            mw.update(cx, |mw, _cx| {
                mw.set_all_groups_expanded(false);
            });
        }
        self.update_entries(cx);
    }

    fn unfold_all(
        &mut self,
        _: &editor::actions::UnfoldAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(mw) = self.multi_workspace.upgrade() {
            mw.update(cx, |mw, _cx| {
                mw.set_all_groups_expanded(true);
            });
        }
        self.update_entries(cx);
    }

    fn activate_workspace(
        &self,
        workspace: &Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(multi_workspace) = self.multi_workspace.upgrade() {
            multi_workspace.update(cx, |mw, cx| {
                mw.activate(workspace.clone(), None, window, cx);
            });
        }
    }

    fn render_recent_projects_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let multi_workspace = self.multi_workspace.upgrade();

        let workspace = multi_workspace
            .as_ref()
            .map(|mw| mw.read(cx).workspace().downgrade());

        let focus_handle = workspace
            .as_ref()
            .and_then(|ws| ws.upgrade())
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<ProjectGroupKey> = multi_workspace
            .as_ref()
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        let popover_handle = self.recent_projects_popover_handle.clone();

        PopoverMenu::new("sidebar-recent-projects-menu")
            .with_handle(popover_handle)
            .menu(move |window, cx| {
                workspace.as_ref().map(|ws| {
                    SidebarRecentProjects::popover(
                        ws.clone(),
                        window_project_groups.clone(),
                        focus_handle.clone(),
                        window,
                        cx,
                    )
                })
            })
            .trigger_with_tooltip(
                IconButton::new("open-project", IconName::FolderAdd)
                    .icon_size(IconSize::Small)
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent)),
                |_window, cx| Tooltip::for_action("Add Project", &OpenRecent::default(), cx),
            )
            .offset(gpui::Point {
                x: px(-2.0),
                y: px(-2.0),
            })
            .anchor(gpui::Anchor::BottomRight)
    }

    fn workspace_for_group(&self, key: &ProjectGroupKey, cx: &App) -> Option<Entity<Workspace>> {
        let mw = self.multi_workspace.upgrade()?;
        let mw = mw.read(cx);
        let active = mw.workspace().clone();
        let active_key = active.read(cx).project_group_key(cx);
        if active_key == *key {
            Some(active)
        } else {
            mw.workspace_for_paths(key.path_list(), key.host().as_ref(), cx)
        }
    }

    pub(crate) fn activate_or_open_workspace_for_group(
        &mut self,
        key: &ProjectGroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self
            .multi_workspace
            .upgrade()
            .and_then(|mw| mw.read(cx).last_active_workspace_for_group(key, cx))
            .or_else(|| self.workspace_for_group(key, cx));
        if let Some(workspace) = workspace {
            if self.is_active_workspace(&workspace, cx) {
                return;
            }
            self.activate_workspace(&workspace, window, cx);
        } else {
            self.open_workspace_for_group(key, window, cx);
        }
        self.selection = None;
    }

    fn active_project_group_key(&self, cx: &App) -> Option<ProjectGroupKey> {
        let multi_workspace = self.multi_workspace.upgrade()?;
        let multi_workspace = multi_workspace.read(cx);
        Some(multi_workspace.project_group_key_for_workspace(multi_workspace.workspace(), cx))
    }

    fn active_project_header_position(&self, cx: &App) -> Option<usize> {
        let active_key = self.active_project_group_key(cx)?;
        self.contents
            .project_header_indices
            .iter()
            .position(|&entry_ix| {
                matches!(
                    &self.contents.entries[entry_ix],
                    ListEntry::ProjectHeader { key, .. } if *key == active_key
                )
            })
    }

    fn cycle_project_impl(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };

        let header_count = self.contents.project_header_indices.len();
        if header_count == 0 {
            return;
        }

        let current_pos = self.active_project_header_position(cx);

        let next_pos = match current_pos {
            Some(pos) => {
                if forward {
                    (pos + 1) % header_count
                } else {
                    (pos + header_count - 1) % header_count
                }
            }
            None => 0,
        };

        let header_entry_ix = self.contents.project_header_indices[next_pos];
        let Some(ListEntry::ProjectHeader { key, .. }) = self.contents.entries.get(header_entry_ix)
        else {
            return;
        };
        let key = key.clone();

        // Uncollapse the target group so that threads become visible.
        self.set_group_expanded(&key, true, cx);

        if let Some(workspace) = self.multi_workspace.upgrade().and_then(|mw| {
            mw.read(cx)
                .workspace_for_paths(key.path_list(), key.host().as_ref(), cx)
        }) {
            multi_workspace.update(cx, |multi_workspace, cx| {
                multi_workspace.activate(workspace, None, window, cx);
                multi_workspace.retain_active_workspace(cx);
            });
        } else {
            self.open_workspace_for_group(&key, window, cx);
        }
    }

    fn on_next_project(&mut self, _: &NextProject, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_project_impl(true, window, cx);
    }

    fn on_previous_project(
        &mut self,
        _: &PreviousProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_project_impl(false, window, cx);
    }

    fn render_no_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = self.has_filter_query(cx);
        let message = if has_query {
            "No threads match your search."
        } else {
            "No threads yet"
        };

        v_flex()
            .id("sidebar-no-results")
            .p_4()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                Label::new(message)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ProjectEmptyState::new(
            "Threads Sidebar",
            self.focus_handle(cx),
            KeyBinding::for_action(&workspace::Open::default(), cx),
        )
        .on_open_project(|_, window, cx| {
            let side = match AgentSettings::get_global(cx).sidebar_side() {
                SidebarSide::Left => "left",
                SidebarSide::Right => "right",
            };
            telemetry::event!("Sidebar Add Project Clicked", side = side);
            window.dispatch_action(
                Open {
                    create_new_window: Some(false),
                }
                .boxed_clone(),
                cx,
            );
        })
        .on_clone_repo(|_, window, cx| {
            window.dispatch_action(git::Clone.boxed_clone(), cx);
        })
    }

    fn render_sidebar_header(
        &self,
        no_open_projects: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sidebar_on_left = self.side(cx) == SidebarSide::Left;
        let sidebar_on_right = self.side(cx) == SidebarSide::Right;
        let not_fullscreen = !window.is_fullscreen() && !window.is_simple_fullscreen();
        let traffic_lights = cfg!(target_os = "macos") && not_fullscreen && sidebar_on_left;
        let left_window_controls = !cfg!(target_os = "macos") && not_fullscreen && sidebar_on_left;
        let right_window_controls =
            !cfg!(target_os = "macos") && not_fullscreen && sidebar_on_right;
        let header_height = platform_title_bar_height(window);

        h_flex()
            .h(header_height)
            .map(|header| match window.window_decorations() {
                // Without projects there's no bottom border to match the title bar's.
                Decorations::Client { .. } => apply_title_bar_insets(
                    header,
                    left_window_controls,
                    right_window_controls,
                    no_open_projects,
                ),
                Decorations::Server => header.mt_px().pb_px(),
            })
            .when(left_window_controls, |this| {
                this.children(Self::render_left_window_controls(window, cx))
            })
            .map(|this| {
                if traffic_lights {
                    this.pl(px(ui::utils::TRAFFIC_LIGHT_PADDING))
                } else if !left_window_controls {
                    this.pl_1p5()
                } else {
                    this
                }
            })
            .when(!right_window_controls, |this| this.pr_1p5())
            .gap_1()
            .when(!no_open_projects, |this| {
                this.border_b_1()
                    .border_color(cx.theme().colors().border)
                    // Fills the space the removed search box used, keeping
                    // right-side window controls at the edge.
                    .child(div().flex_1())
            })
            .when(right_window_controls, |this| {
                this
                    // Without projects, nothing flex-grows to push right-side window controls
                    // to the edge.
                    .when(no_open_projects, |this| this.justify_end())
                    .children(Self::render_right_window_controls(window, cx))
            })
    }

    fn render_left_window_controls(window: &Window, cx: &mut App) -> Option<AnyElement> {
        platform_title_bar::render_left_window_controls(
            cx.button_layout(),
            Box::new(CloseWindow),
            window,
        )
    }

    fn render_right_window_controls(window: &Window, cx: &mut App) -> Option<AnyElement> {
        platform_title_bar::render_right_window_controls(
            cx.button_layout(),
            Box::new(CloseWindow),
            window,
        )
    }

    fn render_sidebar_toggle_button(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let on_right = AgentSettings::get_global(_cx).sidebar_side() == SidebarSide::Right;

        sidebar_side_context_menu("sidebar-toggle-menu", _cx)
            .anchor(if on_right {
                gpui::Anchor::BottomRight
            } else {
                gpui::Anchor::BottomLeft
            })
            .attach(if on_right {
                gpui::Anchor::TopRight
            } else {
                gpui::Anchor::TopLeft
            })
            .trigger(move |_is_active, _window, _cx| {
                let icon = if on_right {
                    IconName::ThreadsSidebarRightOpen
                } else {
                    IconName::ThreadsSidebarLeftOpen
                };
                IconButton::new("sidebar-close-toggle", icon)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::element(move |_window, cx| {
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_between()
                                    .child(Label::new("Toggle Sidebar"))
                                    .child(KeyBinding::for_action(&ToggleWorkspaceSidebar, cx)),
                            )
                            .child(
                                h_flex()
                                    .pt_1()
                                    .gap_2()
                                    .border_t_1()
                                    .border_color(cx.theme().colors().border_variant)
                                    .justify_between()
                                    .child(Label::new("Focus Sidebar"))
                                    .child(KeyBinding::for_action(&FocusWorkspaceSidebar, cx)),
                            )
                            .into_any_element()
                    }))
                    .on_click(|_, window, cx| {
                        if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
                            multi_workspace.update(cx, |multi_workspace, cx| {
                                multi_workspace.close_sidebar(window, cx);
                            });
                        }
                    })
            })
    }

    fn render_sidebar_bottom_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let on_right = self.side(cx) == SidebarSide::Right;

        h_flex()
            .p_1()
            .gap_1()
            .when(on_right, |this| this.flex_row_reverse())
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .child(self.render_sidebar_toggle_button(cx))
            .child(div().flex_1())
            .child(self.render_recent_projects_button(cx))
    }
}

impl WorkspaceSidebar for Sidebar {
    fn width(&self, _cx: &App) -> Pixels {
        self.width
    }

    fn set_width(&mut self, width: Option<Pixels>, cx: &mut Context<Self>) {
        // `None` is the reset gesture, which hands the width back to the setting.
        self.width_set_by_user = width.is_some();
        self.width = width
            .unwrap_or_else(|| AgentSettings::get_global(cx).threads_sidebar.default_width)
            .clamp(THREADS_LIST_MIN_WIDTH, THREADS_LIST_MAX_WIDTH);
        cx.notify();
    }

    fn has_notifications(&self, _cx: &App) -> bool {
        false
    }

    fn is_threads_list_view_active(&self) -> bool {
        true
    }

    fn side(&self, cx: &App) -> SidebarSide {
        AgentSettings::get_global(cx).sidebar_side()
    }

    fn prepare_for_focus(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        cx.notify();
    }

    fn cycle_project(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_project_impl(forward, window, cx);
    }

    fn serialized_state(&self, _cx: &App) -> Option<String> {
        let serialized = SerializedSidebar {
            width: self.width_set_by_user.then(|| f32::from(self.width)),
            width_set_by_user: self.width_set_by_user,
            active_view: SerializedSidebarView::ThreadList,
        };
        serde_json::to_string(&serialized).ok()
    }

    fn restore_serialized_state(
        &mut self,
        state: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(serialized) = serde_json::from_str::<SerializedSidebar>(state).log_err() {
            // Legacy widths other than the old 300-pixel default imply a manual resize
            if let Some(width) = serialized
                .width
                .filter(|width| serialized.width_set_by_user || *width != 300.0)
            {
                self.width = px(width).clamp(THREADS_LIST_MIN_WIDTH, THREADS_LIST_MAX_WIDTH);
                self.width_set_by_user = true;
            }
        }
        cx.notify();
    }
}

impl gpui::EventEmitter<workspace::SidebarEvent> for Sidebar {}

impl Focusable for Sidebar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _titlebar_height = ui::utils::platform_title_bar_height(window);
        let ui_font = theme_settings::setup_ui_font(window, cx);
        let sticky_header = self.render_sticky_header(window, cx);

        let color = cx.theme().colors();
        let bg = color.background.blend(color.panel_background);

        let no_open_projects = !self.contents.has_open_projects;
        let no_search_results = self.contents.entries.is_empty();

        v_flex()
            .id("workspace-sidebar")
            .key_context(self.dispatch_context(window, cx))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::expand_selected_entry))
            .on_action(cx.listener(Self::collapse_selected_entry))
            .on_action(cx.listener(Self::toggle_selected_fold))
            .on_action(cx.listener(Self::fold_all))
            .on_action(cx.listener(Self::unfold_all))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::on_next_project))
            .on_action(cx.listener(Self::on_previous_project))
            .on_action(cx.listener(|this, _: &OpenRecent, window, cx| {
                this.recent_projects_popover_handle.toggle(window, cx);
            }))
            .font(ui_font)
            .map(|el| {
                let on_left = self.side(cx) == SidebarSide::Left;
                match window.window_decorations() {
                    Decorations::Server => el.h_full().w(self.width),
                    // With client-side decorations the sidebar owns the window
                    // corners on its side, so round them like the title bar and
                    // status bar do. The sidebar is stretched 1px outwards over
                    // the window border on untiled edges (with compensating
                    // padding) so its rounded background lines up exactly with
                    // the window shape, avoiding a transparent gap in the
                    // rounded corners.
                    Decorations::Client { tiling, .. } => el
                        .absolute()
                        .top(if tiling.top { px(0.) } else { px(-1.) })
                        .bottom(if tiling.bottom { px(0.) } else { px(-1.) })
                        .when(!tiling.top, |el| el.pt_px())
                        .when(!tiling.bottom, |el| el.pb_px())
                        .map(|el| {
                            if on_left {
                                el.right(px(0.))
                                    .left(if tiling.left { px(0.) } else { px(-1.) })
                                    .when(!tiling.left, |el| el.pl(px(1.)))
                            } else {
                                el.left(px(0.))
                                    .right(if tiling.right { px(0.) } else { px(-1.) })
                                    .when(!tiling.right, |el| el.pr(px(1.)))
                            }
                        })
                        .when(on_left && !(tiling.top || tiling.left), |el| {
                            el.rounded_tl(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(on_left && !(tiling.bottom || tiling.left), |el| {
                            el.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!on_left && !(tiling.top || tiling.right), |el| {
                            el.rounded_tr(CLIENT_SIDE_DECORATION_ROUNDING)
                        })
                        .when(!on_left && !(tiling.bottom || tiling.right), |el| {
                            el.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING)
                        }),
                }
            })
            .bg(bg)
            .when(self.side(cx) == SidebarSide::Left, |el| el.border_r_1())
            .when(self.side(cx) == SidebarSide::Right, |el| el.border_l_1())
            .border_color(color.border)
            .child(self.render_sidebar_header(no_open_projects, window, cx))
            .map(|this| {
                if no_open_projects {
                    this.child(self.render_empty_state(cx))
                } else {
                    this.child(
                        v_flex()
                            .relative()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                list(
                                    self.list_state.clone(),
                                    cx.processor(Self::render_list_entry),
                                )
                                .flex_1()
                                .size_full(),
                            )
                            .when(no_search_results, |this| {
                                this.child(self.render_no_results(cx))
                            })
                            .when_some(sticky_header, |this, header| this.child(header))
                            .custom_scrollbars(
                                Scrollbars::new(ScrollAxes::Vertical)
                                    .tracked_scroll_handle(&self.list_state),
                                window,
                                cx,
                            ),
                    )
                }
            })
            .child(self.render_sidebar_bottom_bar(cx))
    }
}

pub fn dump_workspace_info(
    workspace: &mut Workspace,
    _: &DumpWorkspaceInfo,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Workspace>,
) {
    use std::fmt::Write;

    let mut output = String::new();
    let this_entity = cx.entity();

    let multi_workspace = workspace.multi_workspace().and_then(|weak| weak.upgrade());
    let workspaces: Vec<gpui::Entity<Workspace>> = match &multi_workspace {
        Some(mw) => mw.read(cx).workspaces().cloned().collect(),
        None => vec![this_entity.clone()],
    };
    let active_workspace = multi_workspace
        .as_ref()
        .map(|mw| mw.read(cx).workspace().clone());

    writeln!(output, "MultiWorkspace: {} workspace(s)", workspaces.len()).ok();

    if let Some(mw) = &multi_workspace {
        let keys: Vec<_> = mw.read(cx).project_group_keys();
        writeln!(output, "Project group keys ({}):", keys.len()).ok();
        for key in keys {
            writeln!(output, "  - {key:?}").ok();
        }
    }

    writeln!(output).ok();

    for (index, ws) in workspaces.iter().enumerate() {
        let is_active = active_workspace.as_ref() == Some(ws);
        writeln!(
            output,
            "--- Workspace {index}{} ---",
            if is_active { " (active)" } else { "" }
        )
        .ok();

        // project_group_key_for_workspace internally reads the workspace,
        // so we can only call it for workspaces other than this_entity
        // (which is already being updated).
        if let Some(mw) = &multi_workspace {
            if *ws == this_entity {
                let workspace_key = workspace.project_group_key(cx);
                writeln!(output, "ProjectGroupKey: {workspace_key:?}").ok();
            } else {
                let effective_key = mw.read(cx).project_group_key_for_workspace(ws, cx);
                let workspace_key = ws.read(cx).project_group_key(cx);
                if effective_key != workspace_key {
                    writeln!(
                        output,
                        "ProjectGroupKey (multi_workspace): {effective_key:?}"
                    )
                    .ok();
                    writeln!(
                        output,
                        "ProjectGroupKey (workspace, DISAGREES): {workspace_key:?}"
                    )
                    .ok();
                } else {
                    writeln!(output, "ProjectGroupKey: {effective_key:?}").ok();
                }
            }
        } else {
            let workspace_key = workspace.project_group_key(cx);
            writeln!(output, "ProjectGroupKey: {workspace_key:?}").ok();
        }

        // The action handler is already inside an update on `this_entity`,
        // so we must avoid a nested read/update on that same entity.
        if *ws == this_entity {
            dump_single_workspace(workspace, &mut output, cx);
        } else {
            ws.read_with(cx, |ws, cx| {
                dump_single_workspace(ws, &mut output, cx);
            });
        }
    }

    let project = workspace.project().clone();
    cx.spawn_in(window, async move |_this, cx| {
        let buffer = project
            .update(cx, |project, cx| project.create_buffer(None, false, cx))
            .await?;

        buffer.update(cx, |buffer, cx| {
            buffer.set_text(output, cx);
        });

        let buffer = cx.new(|cx| {
            editor::MultiBuffer::singleton(buffer, cx).with_title("Workspace Info".into())
        });

        _this.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(
                Box::new(cx.new(|cx| {
                    let mut editor =
                        editor::Editor::for_multibuffer(buffer, Some(project.clone()), window, cx);
                    editor.set_read_only(true);
                    editor.set_should_serialize(false, cx);
                    editor.set_breadcrumb_header("Workspace Info".into());
                    editor
                })),
                None,
                true,
                window,
                cx,
            );
        })
    })
    .detach_and_log_err(cx);
}

fn dump_single_workspace(workspace: &Workspace, output: &mut String, cx: &gpui::App) {
    use std::fmt::Write;

    let workspace_db_id = workspace.database_id();
    match workspace_db_id {
        Some(id) => writeln!(output, "Workspace DB ID: {id:?}").ok(),
        None => writeln!(output, "Workspace DB ID: (none)").ok(),
    };

    let project = workspace.project().read(cx);

    let repos: Vec<_> = project
        .repositories(cx)
        .values()
        .map(|repo| repo.read(cx).snapshot())
        .collect();

    writeln!(output, "Worktrees:").ok();
    for worktree in project.worktrees(cx) {
        let worktree = worktree.read(cx);
        let abs_path = worktree.abs_path();
        let visible = worktree.is_visible();

        let repo_info = repos
            .iter()
            .find(|snapshot| abs_path.starts_with(&*snapshot.work_directory_abs_path));

        let is_linked = repo_info.map(|s| s.is_linked_worktree()).unwrap_or(false);
        let main_worktree_path = repo_info.and_then(|s| s.main_worktree_abs_path());
        let branch = repo_info.and_then(|s| s.branch.as_ref().map(|b| b.ref_name.clone()));

        write!(output, "  - {}", abs_path.display()).ok();
        if !visible {
            write!(output, " (hidden)").ok();
        }
        if let Some(branch) = &branch {
            write!(output, " [branch: {branch}]").ok();
        }
        if is_linked {
            if let Some(main_worktree_path) = main_worktree_path {
                write!(
                    output,
                    " [linked worktree -> {}]",
                    main_worktree_path.display()
                )
                .ok();
            } else {
                write!(output, " [linked worktree]").ok();
            }
        }
        writeln!(output).ok();
    }

    writeln!(output).ok();
}
