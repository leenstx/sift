use super::*;
use fs::FakeFs;
use gpui::{App, Hsla, TestAppContext, UpdateGlobal, VisualTestContext};
use pretty_assertions::assert_eq;
use settings::SettingsStore;
use std::collections::HashSet;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use util::path_list::PathList;

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        // Use an isolated DB so parallel tests can't see each other's
        // persisted records (e.g. created-worktree records).
        cx.set_global(db::AppDatabase::test_new());
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
    });
}

async fn init_test_project(
    worktree_path: &str,
    cx: &mut TestAppContext,
) -> Entity<project::Project> {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(worktree_path, serde_json::json!({ "src": {} }))
        .await;
    cx.update(|cx| <dyn fs::Fs>::set_global(fs.clone(), cx));
    project::Project::test(fs, [worktree_path.as_ref()], cx).await
}

#[gpui::test]
async fn test_workspace_menu_uses_bare_repository_worktree_name(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        "/zed/.bare",
        serde_json::json!({
            "worktrees": {
                "glossy-walrus": {
                    "commondir": "../..",
                    "HEAD": "ref: refs/heads/glossy-walrus",
                },
            },
        }),
    )
    .await;
    fs.insert_tree(
        "/worktrees/zed/glossy-walrus/zed",
        serde_json::json!({
            ".git": "gitdir: /zed/.bare/worktrees/glossy-walrus",
            "src": {},
        }),
    )
    .await;
    cx.update(|cx| <dyn fs::Fs>::set_global(fs.clone(), cx));

    let project =
        project::Project::test(fs, [Path::new("/worktrees/zed/glossy-walrus/zed")], cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });
    let labels = cx.update(|_window, cx| workspace_menu_worktree_labels(&workspace, cx));

    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].primary_name.as_ref(), "glossy-walrus");
    assert_eq!(labels[0].secondary_name, None);
}

fn setup_sidebar(
    multi_workspace: &Entity<MultiWorkspace>,
    cx: &mut gpui::VisualTestContext,
) -> Entity<Sidebar> {
    let sidebar = setup_sidebar_closed(multi_workspace, cx);
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.toggle_sidebar(window, cx);
    });
    cx.run_until_parked();
    sidebar
}

fn setup_sidebar_closed(
    multi_workspace: &Entity<MultiWorkspace>,
    cx: &mut gpui::VisualTestContext,
) -> Entity<Sidebar> {
    let multi_workspace = multi_workspace.clone();
    let sidebar =
        cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
    multi_workspace.update(cx, |mw, cx| {
        mw.register_sidebar(sidebar.clone(), cx);
    });
    cx.run_until_parked();
    sidebar
}

fn set_threads_sidebar_default_width(width: f32, cx: &mut App) {
    SettingsStore::update_global(cx, |store, cx| {
        store
            .set_user_settings(
                &format!(r#"{{"agent": {{"threads_sidebar": {{"default_width": {width}}}}}}}"#),
                cx,
            )
            .unwrap();
    });
}

fn focus_sidebar(sidebar: &Entity<Sidebar>, cx: &mut gpui::VisualTestContext) {
    sidebar.update_in(cx, |_, window, cx| {
        cx.focus_self(window);
    });
    cx.run_until_parked();
}

fn visible_entries_as_strings(
    sidebar: &Entity<Sidebar>,
    cx: &mut gpui::VisualTestContext,
) -> Vec<String> {
    sidebar.read_with(cx, |sidebar, cx| {
        sidebar
            .contents
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let selected = if sidebar.selection == Some(ix) {
                    "  <== selected"
                } else {
                    ""
                };
                match entry {
                    ListEntry::ProjectHeader { label, key, .. } => {
                        let icon = if sidebar.is_group_collapsed(key, cx) {
                            ">"
                        } else {
                            "v"
                        };
                        format!("{} [{}]{}", icon, label, selected)
                    }
                }
            })
            .collect()
    })
}

#[gpui::test]
async fn test_sidebar_background_with_transparent_panel(cx: &mut TestAppContext) {
    let (_, project) = init_multi_project_test(&["/my-project"], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    for panel_alpha in [0.0, 0.4] {
        set_sidebar_test_panel_alpha(panel_alpha, cx);
        let background = cx.update(|_, cx| {
            let colors = cx.theme().colors();
            colors.background.blend(colors.panel_background)
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(400.), px(240.)),
            |_, _| sidebar.clone().into_any_element(),
        );

        cx.update(|window, _| {
            let quads = window.painted_quads();
            let sidebar_background = quads.first().expect("Sidebar should paint its background");
            assert_eq!(sidebar_background.background.as_solid(), Some(background));
        });
    }
}

#[gpui::test]
async fn test_width_reset_returns_configured_default(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    cx.update(|cx| set_threads_sidebar_default_width(360.0, cx));
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.width),
        px(360.0),
        "a fresh sidebar should open at the configured width"
    );

    sidebar.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(Some(px(420.0)), cx);
    });
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), px(420.0));

    sidebar.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(None, cx);
    });
    cx.run_until_parked();

    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.width),
        px(360.0),
        "resetting the width should return to the configured width, not a fixed default"
    );
}

#[gpui::test]
async fn test_width_setting_overrides_manual_resize(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    cx.update(|_window, cx| set_threads_sidebar_default_width(360.0, cx));
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), px(360.0));
    assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.width_set_by_user));

    sidebar.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(Some(px(420.0)), cx);
    });
    cx.update(|_window, cx| {
        SettingsStore::update_global(cx, |store, cx| {
            store
                .set_user_settings(
                    r#"{"agent":{"threads_sidebar":{"default_width":360,"position":"right"}}}"#,
                    cx,
                )
                .expect("settings are valid");
        });
    });
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), px(420.0));
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.width_set_by_user));

    cx.update(|_window, cx| set_threads_sidebar_default_width(500.0, cx));
    cx.run_until_parked();
    assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), px(500.0));
    assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.width_set_by_user));

    for (configured, expected) in [
        (5.0, THREADS_LIST_MIN_WIDTH),
        (360.0, px(360.0)),
        (5000.0, THREADS_LIST_MAX_WIDTH),
    ] {
        cx.update(|_window, cx| set_threads_sidebar_default_width(configured, cx));
        cx.run_until_parked();
        assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), expected);
        let serialized = sidebar
            .read_with(cx, |sidebar, cx| sidebar.serialized_state(cx))
            .expect("sidebar state is serialized");
        let serialized: SerializedSidebar =
            serde_json::from_str(&serialized).expect("sidebar state is valid");
        assert_eq!(serialized.width, None);
        assert!(!serialized.width_set_by_user);
    }
}

#[gpui::test]
async fn test_restored_width_preserves_legacy_resizes(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

    for (state, expected, width_set_by_user) in [
        (r#"{"width":null}"#, 360.0, false),
        (r#"{"width":300.0}"#, 360.0, false),
        (r#"{"width":420.0}"#, 420.0, true),
        (r#"{"width":300.0,"width_set_by_user":true}"#, 300.0, true),
    ] {
        cx.update(|_window, cx| set_threads_sidebar_default_width(360.0, cx));
        let sidebar =
            cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
        cx.run_until_parked();
        sidebar.update_in(cx, |sidebar, window, cx| {
            sidebar.restore_serialized_state(state, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.width),
            px(expected)
        );
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.width_set_by_user),
            width_set_by_user
        );

        let serialized = sidebar
            .read_with(cx, |sidebar, cx| sidebar.serialized_state(cx))
            .expect("sidebar state is serialized");
        let serialized: SerializedSidebar =
            serde_json::from_str(&serialized).expect("sidebar state is valid");
        assert_eq!(serialized.width, width_set_by_user.then_some(expected));
        assert_eq!(serialized.width_set_by_user, width_set_by_user);

        cx.update(|_window, cx| set_threads_sidebar_default_width(500.0, cx));
        cx.run_until_parked();
        assert_eq!(sidebar.read_with(cx, |sidebar, _| sidebar.width), px(500.0));
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.width_set_by_user));
    }
}

#[gpui::test]
async fn test_configured_width_is_clamped_into_range(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    cx.update(|cx| set_threads_sidebar_default_width(5000.0, cx));
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

    let wide_sidebar =
        cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
    cx.run_until_parked();
    assert_eq!(
        wide_sidebar.read_with(cx, |sidebar, _| sidebar.width),
        THREADS_LIST_MAX_WIDTH,
        "a configured width above the maximum should be clamped at construction"
    );

    wide_sidebar.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(None, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        wide_sidebar.read_with(cx, |sidebar, _| sidebar.width),
        THREADS_LIST_MAX_WIDTH,
        "resetting the width should not restore the unclamped configured width"
    );

    cx.update(|_window, cx| set_threads_sidebar_default_width(5.0, cx));
    let narrow_sidebar =
        cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
    cx.run_until_parked();
    assert_eq!(
        narrow_sidebar.read_with(cx, |sidebar, _| sidebar.width),
        THREADS_LIST_MIN_WIDTH,
        "a configured width below the minimum should be clamped at construction"
    );
}

#[gpui::test]
async fn test_only_a_user_chosen_width_is_persisted(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    cx.update(|cx| set_threads_sidebar_default_width(360.0, cx));
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    // Serialization runs on many triggers besides resizing, so a sidebar the
    // user has never resized must not record a width at all.
    let untouched = sidebar
        .read_with(cx, |sidebar, cx| sidebar.serialized_state(cx))
        .expect("serialized_state should return Some");
    assert!(
        !untouched.contains("360"),
        "an unresized sidebar should not persist its width, got {untouched}"
    );

    // A legacy width matching the old default is ambiguous, so the setting takes precedence
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.restore_serialized_state(r#"{"width":300.0}"#, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        sidebar.read_with(cx, |sidebar, _| sidebar.width),
        px(360.0),
        "the legacy default width falls back to the setting"
    );

    // A width the user picked survives, and keeps surviving across restores.
    sidebar.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(Some(px(420.0)), cx);
    });
    cx.run_until_parked();
    let resized = sidebar
        .read_with(cx, |sidebar, cx| sidebar.serialized_state(cx))
        .expect("serialized_state should return Some");

    let restored =
        cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
    cx.run_until_parked();
    restored.update_in(cx, |sidebar, window, cx| {
        sidebar.restore_serialized_state(&resized, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        restored.read_with(cx, |sidebar, _| sidebar.width),
        px(420.0),
        "a user-chosen width should survive a restore"
    );

    // Resetting gives the width back to the setting, and stops persisting it.
    restored.update_in(cx, |sidebar, _window, cx| {
        sidebar.set_width(None, cx);
    });
    cx.run_until_parked();
    let after_reset = restored
        .read_with(cx, |sidebar, cx| sidebar.serialized_state(cx))
        .expect("serialized_state should return Some");
    assert!(
        !after_reset.contains("420"),
        "resetting should stop persisting the width, got {after_reset}"
    );
}

#[gpui::test]
async fn test_entities_released_on_window_close(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    let weak_workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().downgrade());
    let weak_sidebar = sidebar.downgrade();
    let weak_multi_workspace = multi_workspace.downgrade();

    drop(sidebar);
    drop(multi_workspace);
    cx.update(|window, _cx| window.remove_window());
    cx.run_until_parked();

    weak_multi_workspace.assert_released();
    weak_sidebar.assert_released();
    weak_workspace.assert_released();
}

#[gpui::test]
async fn test_refocus_sidebar_with_no_selection_focuses_list(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);
    let workspace = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();
    sidebar.update_in(cx, |sidebar, window, cx| {
        assert!(!sidebar.focus_handle.is_focused(window));
        assert!(!sidebar.filter_editor.read(cx).is_focused(window));
        assert_eq!(sidebar.selection, None);
    });

    // There is no search box, so refocusing with no selection keeps focus on
    // the list without selecting a row.
    focus_sidebar(&sidebar, cx);
    sidebar.update_in(cx, |sidebar, window, cx| {
        assert!(sidebar.focus_handle.is_focused(window));
        assert!(!sidebar.filter_editor.read(cx).is_focused(window));
        assert_eq!(sidebar.selection, None);
    });
}

#[gpui::test]
async fn test_refocus_sidebar_with_selection_preserves_it(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);
    let workspace = multi_workspace.read_with(cx, |multi_workspace, _cx| {
        multi_workspace.workspace().clone()
    });

    sidebar.update(cx, |sidebar, cx| {
        sidebar.selection = Some(0);
        cx.notify();
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();

    // Refocusing with a selection returns focus to the list and keeps that row selected.
    focus_sidebar(&sidebar, cx);
    sidebar.update_in(cx, |sidebar, window, _cx| {
        assert!(sidebar.focus_handle.is_focused(window));
        assert_eq!(sidebar.selection, Some(0));
    });
}

// Reproduces the core of the user-reported bug: a thread belonging to
// a multi-root workspace that mixes a standalone project and a linked
// git worktree can become invisible in the sidebar when its stored
// `main_worktree_paths` don't match the workspace's project group
// key. The metadata still exists and Thread History still shows it,
// but the sidebar rebuild's lookups all miss.
//
// Real-world setup: a single multi-root workspace whose roots are
// `[/cloud, /worktrees/zed/wt_a/zed]`, where:
//   - `/cloud` is a standalone git repo (main == folder).
//   - `/worktrees/zed/wt_a/zed` is a linked worktree of `/zed`.
//
// Once git scans complete the project group key is
// `[/cloud, /zed]` — the main paths of the two roots. A thread
// created in this workspace is written with
// `main=[/cloud, /zed], folder=[/cloud, /worktrees/zed/wt_a/zed]`
// and the sidebar finds it via `entries_for_main_worktree_path`.
//
// If some other code path (stale data on reload, a path-less archive
// restored via the project picker, a legacy write …) persists the
// thread with `main == folder` instead, the stored
// `main_worktree_paths` is
// `[/cloud, /worktrees/zed/wt_a/zed]` ≠ `[/cloud, /zed]`. The three
// lookups in `rebuild_contents` all miss:
//
//   1. `entries_for_main_worktree_path([/cloud, /zed])` — the
//      thread's stored main doesn't equal the group key.
//   2. `entries_for_path([/cloud, /zed])` — the thread's folder paths
//      don't equal the group key either.
//   3. The linked-worktree fallback iterates the group's workspaces'
//      `linked_worktrees()` snapshots. Those yield *sibling* linked
//      worktrees of the repo, not the workspace's own roots, so the
//      thread's folder `/worktrees/zed/wt_a/zed` doesn't match.
//
// The row falls out of the sidebar entirely — matching the user's
// symptom of a thread visible in the agent panel but missing from
// the sidebar. It only reappears once something re-writes the
// thread's metadata in the good shape (e.g. `handle_conversation_event`
// firing after the user sends a message).
//
// We directly persist the bad shape via `store.save(...)` rather
// than trying to reproduce the original writer. The bug is
// ultimately about the sidebar's tolerance for any stale row whose
// folder paths correspond to an open workspace's roots, regardless
// of how that row came to be in the store.

async fn init_multi_project_test(
    paths: &[&str],
    cx: &mut TestAppContext,
) -> (Arc<FakeFs>, Entity<project::Project>) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    for path in paths {
        fs.insert_tree(path, serde_json::json!({ ".git": {}, "src": {} }))
            .await;
    }
    cx.update(|cx| <dyn fs::Fs>::set_global(fs.clone(), cx));
    let project =
        project::Project::test(fs.clone() as Arc<dyn fs::Fs>, [paths[0].as_ref()], cx).await;
    (fs, project)
}

async fn add_test_project(
    path: &str,
    fs: &Arc<FakeFs>,
    multi_workspace: &Entity<MultiWorkspace>,
    cx: &mut gpui::VisualTestContext,
) -> Entity<Workspace> {
    let project = project::Project::test(fs.clone() as Arc<dyn fs::Fs>, [path.as_ref()], cx).await;
    let workspace = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project, window, cx)
    });
    cx.run_until_parked();
    workspace
}

#[gpui::test]
async fn test_workspace_lifecycle_retains_projects_when_sidebar_is_closed(cx: &mut TestAppContext) {
    let (fs, project_a) =
        init_multi_project_test(&["/project-a", "/project-b", "/project-c"], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let _sidebar = setup_sidebar_closed(&multi_workspace, cx);

    let workspace_a = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    assert!(!multi_workspace.read_with(cx, |mw, _| mw.sidebar_open()));
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        1
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_a));

    let workspace_b = add_test_project("/project-b", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_b));
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_a)));

    let workspace_c = add_test_project("/project-c", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        3
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_c));
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_b)));
}

#[gpui::test]
async fn test_workspaces_remain_retained_after_sidebar_closes(cx: &mut TestAppContext) {
    let (fs, project_a) = init_multi_project_test(
        &["/project-a", "/project-b", "/project-c", "/project-d"],
        cx,
    )
    .await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    let _sidebar = setup_sidebar(&multi_workspace, cx);
    assert!(multi_workspace.read_with(cx, |mw, _| mw.sidebar_open()));
    let workspace_a = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    let workspace_b = add_test_project("/project-b", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.activate(workspace_a, None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_b)));

    multi_workspace.update_in(cx, |mw, window, cx| mw.close_sidebar(window, cx));
    cx.run_until_parked();
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );

    let workspace_c = add_test_project("/project-c", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        3
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_c));

    let workspace_d = add_test_project("/project-d", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        4
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_d));
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_c)));
}

#[gpui::test]
async fn test_sidebar_opening_keeps_existing_retained_workspaces(cx: &mut TestAppContext) {
    let (fs, project_a) =
        init_multi_project_test(&["/project-a", "/project-b", "/project-c"], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project_a, window, cx));
    setup_sidebar_closed(&multi_workspace, cx);

    let workspace_a = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let workspace_b = add_test_project("/project-b", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_b));
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_a)));

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.toggle_sidebar(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        2
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspaces().any(|w| w == &workspace_b)));

    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.toggle_sidebar(window, cx);
    });

    let workspace_c = add_test_project("/project-c", &fs, &multi_workspace, cx).await;
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspaces().count()),
        3
    );
    assert!(multi_workspace.read_with(cx, |mw, _| mw.workspace() == &workspace_c));
}

#[gpui::test]
async fn test_linked_worktree_workspace_reachable_after_adding_worktree_to_project(
    cx: &mut TestAppContext,
) {
    // When a linked worktree is opened as its own workspace and then a new
    // folder is added to the main project group, the linked worktree
    // workspace must still be reachable from some sidebar entry.
    let (_fs, project) = init_multi_project_test(&["/my-project"], cx).await;
    let fs = _fs.clone();

    // Set up git worktree infrastructure.
    fs.insert_tree(
        "/my-project/.git/worktrees/wt-0",
        serde_json::json!({
            "commondir": "../../",
            "HEAD": "ref: refs/heads/wt-0",
        }),
    )
    .await;
    fs.insert_tree(
        "/worktrees/wt-0",
        serde_json::json!({
            ".git": "gitdir: /my-project/.git/worktrees/wt-0",
            "src": {},
        }),
    )
    .await;
    fs.add_linked_worktree_for_repo(
        Path::new("/my-project/.git"),
        false,
        git::repository::Worktree {
            path: PathBuf::from("/worktrees/wt-0"),
            ref_name: Some("refs/heads/wt-0".into()),
            sha: "aaa".into(),
            is_main: false,
            is_bare: false,
        },
    )
    .await;

    // Re-scan so the main project discovers the linked worktree.
    project.update(cx, |p, cx| p.git_scans_complete(cx)).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    // Open the linked worktree as its own workspace.
    let worktree_project = project::Project::test(
        fs.clone() as Arc<dyn fs::Fs>,
        ["/worktrees/wt-0".as_ref()],
        cx,
    )
    .await;
    worktree_project
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(worktree_project.clone(), window, cx);
    });
    cx.run_until_parked();

    // Both workspaces should be reachable.
    let workspace_count = multi_workspace.read_with(cx, |mw, _| mw.workspaces().count());
    assert_eq!(workspace_count, 2, "should have 2 workspaces");

    // Add a new folder to the main project, changing the project group key.
    fs.insert_tree(
        "/other-project",
        serde_json::json!({ ".git": {}, "src": {} }),
    )
    .await;
    project
        .update(cx, |project, cx| {
            project.find_or_create_worktree("/other-project", true, cx)
        })
        .await
        .expect("should add worktree");
    cx.run_until_parked();

    sidebar.update_in(cx, |sidebar, _window, cx| sidebar.update_entries(cx));
    cx.run_until_parked();

    // The linked worktree workspace must still be reachable.
    let entries = visible_entries_as_strings(&sidebar, cx);
    let mw_workspaces: Vec<_> = multi_workspace.read_with(cx, |mw, _| {
        mw.workspaces().map(|ws| ws.entity_id()).collect()
    });
    sidebar.read_with(cx, |sidebar, cx| {
        let multi_workspace = multi_workspace.read(cx);
        let reachable: std::collections::HashSet<gpui::EntityId> = sidebar
            .contents
            .entries
            .iter()
            .flat_map(|entry| entry.reachable_workspaces(multi_workspace, cx))
            .map(|ws| ws.entity_id())
            .collect();
        let all: std::collections::HashSet<gpui::EntityId> =
            mw_workspaces.iter().copied().collect();
        let unreachable = &all - &reachable;
        assert!(
            unreachable.is_empty(),
            "all workspaces should be reachable after adding folder; \
             unreachable: {:?}, entries: {:?}",
            unreachable,
            entries,
        );
    });
}

#[gpui::test]
async fn test_cmd_click_project_header_returns_to_last_active_linked_worktree_workspace(
    cx: &mut TestAppContext,
) {
    // Regression test for: cmd-clicking a project group header should return
    // the user to the workspace they most recently had active in that group,
    // including workspaces rooted at a linked worktree.
    init_test(cx);
    let fs = FakeFs::new(cx.executor());

    fs.insert_tree(
        "/project-a",
        serde_json::json!({
            ".git": {},
            "src": {},
        }),
    )
    .await;
    fs.insert_tree("/project-b", serde_json::json!({ "src": {} }))
        .await;

    fs.add_linked_worktree_for_repo(
        Path::new("/project-a/.git"),
        false,
        git::repository::Worktree {
            path: std::path::PathBuf::from("/wt-feature-a"),
            ref_name: Some("refs/heads/feature-a".into()),
            sha: "aaa".into(),
            is_main: false,
            is_bare: false,
        },
    )
    .await;

    cx.update(|cx| <dyn fs::Fs>::set_global(fs.clone(), cx));

    let main_project_a = project::Project::test(fs.clone(), ["/project-a".as_ref()], cx).await;
    let worktree_project_a =
        project::Project::test(fs.clone(), ["/wt-feature-a".as_ref()], cx).await;
    let project_b = project::Project::test(fs.clone(), ["/project-b".as_ref()], cx).await;

    main_project_a
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;
    worktree_project_a
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;

    // The multi-workspace starts with the main-paths workspace of group A
    // as the initially active workspace.
    let (multi_workspace, cx) = cx
        .add_window_view(|window, cx| MultiWorkspace::test_new(main_project_a.clone(), window, cx));

    let sidebar = setup_sidebar(&multi_workspace, cx);

    // Capture the initially active workspace (group A's main-paths workspace)
    // *before* registering additional workspaces, since `workspaces()` returns
    // retained workspaces in registration order — not activation order — and
    // the multi-workspace's starting workspace may not be retained yet.
    let main_workspace_a = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    // Register the linked-worktree workspace (group A) and the group-B
    // workspace. Both get retained by the multi-workspace.
    let worktree_workspace_a = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(worktree_project_a.clone(), window, cx)
    });
    let workspace_b = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(project_b.clone(), window, cx)
    });

    cx.run_until_parked();

    // Step 1: activate the linked-worktree workspace. The MultiWorkspace
    // records this as the last-active workspace for group A on its
    // ProjectGroupState. (We don't assert on the initial active workspace
    // because `test_add_workspace` may auto-activate newly registered
    // workspaces — what matters for this test is the explicit sequence of
    // activations below.)
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.activate(worktree_workspace_a.clone(), None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspace().clone()),
        worktree_workspace_a,
        "linked-worktree workspace should be active after step 1"
    );

    // Step 2: switch to the workspace for group B. Group A's last-active
    // workspace remains the linked-worktree one (group B getting activated
    // records *its own* last-active workspace, not group A's).
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.activate(workspace_b.clone(), None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspace().clone()),
        workspace_b,
        "group B's workspace should be active after step 2"
    );

    // Step 3: simulate cmd-click on group A's header. The project group key
    // for group A is derived from the *main-paths* workspace (linked-worktree
    // workspaces share the same key because it normalizes to main-worktree
    // paths).
    let group_a_key = main_workspace_a.read_with(cx, |ws, cx| ws.project_group_key(cx));
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.activate_or_open_workspace_for_group(&group_a_key, window, cx);
    });
    cx.run_until_parked();

    // Expected: we're back in the linked-worktree workspace, not the
    // main-paths one.
    let active_after_cmd_click = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    assert_eq!(
        active_after_cmd_click, worktree_workspace_a,
        "cmd-click on group A's header should return to the last-active \
         linked-worktree workspace, not the main-paths workspace"
    );
    assert_ne!(
        active_after_cmd_click, main_workspace_a,
        "cmd-click must not fall back to the main-paths workspace when a \
         linked-worktree workspace was the last-active one for the group"
    );
}

#[gpui::test]
async fn test_find_or_create_workspace_returns_the_created_remote_workspace(
    cx: &mut TestAppContext,
    server_cx: &mut TestAppContext,
) {
    let local_project = init_test_project("/local", cx).await;
    cx.update(|cx| {
        release_channel::init(semver::Version::new(0, 0, 0), cx);
    });
    server_cx.update(|cx| {
        release_channel::init(semver::Version::new(0, 0, 0), cx);
    });
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(local_project, window, cx));
    let local_workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    let server_fs = FakeFs::new(server_cx.executor());
    server_fs
        .insert_tree("/remote-project", serde_json::json!({ "src": {} }))
        .await;
    let (opts, server_session, _) = remote::RemoteClient::fake_server(cx, server_cx);
    server_cx.update(remote_server::HeadlessProject::init);
    let server_executor = server_cx.executor();
    let _headless = server_cx.new(|cx| {
        remote_server::HeadlessProject::new(
            remote_server::HeadlessAppState {
                session: server_session,
                fs: server_fs.clone(),
                http_client: Arc::new(http_client::BlockedHttpClient),
                node_runtime: node_runtime::NodeRuntime::unavailable(),
                languages: Arc::new(language::LanguageRegistry::new(server_executor)),
                extension_host_proxy: Arc::new(extension::ExtensionHostProxy::new()),
                startup_time: std::time::Instant::now(),
            },
            false,
            cx,
        )
    });
    let remote_client = remote::RemoteClient::connect_mock(opts.clone(), cx).await;

    // Stand in for the save prompt from a concurrent workspace removal: as
    // soon as the remote workspace is activated mid-open, activate the local
    // workspace again. The open must still return the workspace it created,
    // not whichever workspace is active once it finishes.
    multi_workspace.update_in(cx, |_, window, cx| {
        let local_workspace = local_workspace.clone();
        cx.subscribe_in(&cx.entity(), window, move |this, _, event, window, cx| {
            if matches!(event, MultiWorkspaceEvent::WorkspaceAdded(_)) {
                this.activate(local_workspace.clone(), None, window, cx);
            }
        })
        .detach();
    });

    let created = multi_workspace
        .update_in(cx, |mw, window, cx| {
            let key = ProjectGroupKey::new(
                Some(opts.clone()),
                PathList::new(&[PathBuf::from("/remote-project")]),
            );
            mw.find_or_create_workspace(
                PathList::new(&[PathBuf::from("/remote-project")]),
                Some(opts),
                Some(key),
                move |_, _, _| Task::ready(Ok(Some(remote_client))),
                None,
                workspace::OpenMode::Activate,
                None,
                window,
                cx,
            )
        })
        .await
        .expect("opening the remote project should succeed");
    cx.run_until_parked();

    assert_eq!(
        created.read_with(cx, |workspace, cx| PathList::new(&workspace.root_paths(cx))),
        PathList::new(&[PathBuf::from("/remote-project")]),
        "the returned workspace should be the remote workspace that was created"
    );
    assert_eq!(
        multi_workspace.read_with(cx, |mw, _| mw.workspace().clone()),
        local_workspace,
        "the local workspace should have re-activated during the open"
    );
}

fn set_sidebar_test_panel_alpha(alpha: f32, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let mut theme = cx.theme().as_ref().clone();
        theme.styles.colors.background = Hsla::from(gpui::rgb(0xdcdcdd));
        theme.styles.colors.surface_background = Hsla::from(gpui::rgb(0xff00ff));
        theme.styles.colors.panel_background = Hsla::from(gpui::rgb(0xebebec)).alpha(alpha);
        theme.styles.colors.element_background = Hsla::from(gpui::rgb(0xebebec));
        theme.styles.colors.ghost_element_hover = Hsla::from(gpui::rgb(0xdfdfe0));
        theme::GlobalTheme::update_theme(cx, Arc::new(theme));
        window.refresh();
    });
    cx.run_until_parked();
}
#[gpui::test]
async fn test_collapse_changes_entry_shape(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    cx.run_until_parked();

    let project_group_key = project.read_with(cx, |project, cx| project.project_group_key(cx));

    let before = sidebar.read_with(cx, |sidebar, app| {
        sidebar
            .entry_shapes(multi_workspace.read(app))
            .collect::<Vec<_>>()
    });
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.toggle_collapse(&project_group_key, window, cx);
    });
    cx.run_until_parked();
    let after = sidebar.read_with(cx, |sidebar, app| {
        sidebar
            .entry_shapes(multi_workspace.read(app))
            .collect::<Vec<_>>()
    });

    assert_ne!(
        before, after,
        "collapsing the project group should change the shape sequence so the list resets"
    );
}

#[gpui::test]
async fn test_keyboard_navigation_on_empty_list(cx: &mut TestAppContext) {
    let project = init_test_project("/empty-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    // An empty project has only its header.
    assert_eq!(
        visible_entries_as_strings(&sidebar, cx),
        vec!["v [empty-project]"]
    );

    // Focus sidebar — focus_in does not set a selection
    focus_sidebar(&sidebar, cx);
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection), None);

    // First SelectNext from None starts at index 0 (header)
    cx.dispatch_action(SelectNext);
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection), Some(0));

    // SelectNext with only one entry stays at index 0
    cx.dispatch_action(SelectNext);
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection), Some(0));

    // SelectPrevious from first entry clears selection (returns to editor)
    cx.dispatch_action(SelectPrevious);
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection), None);

    // SelectPrevious from None selects the last entry
    cx.dispatch_action(SelectPrevious);
    assert_eq!(sidebar.read_with(cx, |s, _| s.selection), Some(0));
}

#[gpui::test]
async fn test_serialization_round_trip(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    let project_group_key = project.read_with(cx, |project, cx| project.project_group_key(cx));

    // Set a custom width and collapse the group.
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.set_width(Some(px(420.0)), cx);
        sidebar.toggle_collapse(&project_group_key, window, cx);
    });
    cx.run_until_parked();

    // Capture the serialized state from the first sidebar.
    let serialized = sidebar.read_with(cx, |sidebar, cx| sidebar.serialized_state(cx));
    let serialized = serialized.expect("serialized_state should return Some");

    // Create a fresh sidebar and restore into it.
    let sidebar2 =
        cx.update(|window, cx| cx.new(|cx| Sidebar::new(multi_workspace.clone(), window, cx)));
    cx.run_until_parked();

    sidebar2.update_in(cx, |sidebar, window, cx| {
        sidebar.restore_serialized_state(&serialized, window, cx);
    });
    cx.run_until_parked();

    // Assert all serialized fields match.
    let width1 = sidebar.read_with(cx, |s, _| s.width);
    let width2 = sidebar2.read_with(cx, |s, _| s.width);

    assert_eq!(width1, width2);
    assert_eq!(width1, px(420.0));
}

#[gpui::test]
async fn test_single_workspace_lists_project(cx: &mut TestAppContext) {
    let project = init_test_project("/my-project", cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    assert_eq!(
        visible_entries_as_strings(&sidebar, cx),
        vec!["v [my-project]"]
    );
}

#[gpui::test]
async fn test_linked_worktree_workspace_reachable_after_adding_unrelated_project(
    cx: &mut TestAppContext,
) {
    // Regression test for a property-test finding:
    //   AddLinkedWorktree { project_group_index: 0 }
    //   AddProject { use_worktree: true }
    //   AddProject { use_worktree: false }
    // After these three steps, the linked-worktree workspace was not
    // reachable from any sidebar entry.
    init_test(cx);

    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        "/my-project",
        serde_json::json!({
            ".git": {},
            "src": {},
        }),
    )
    .await;
    cx.update(|cx| <dyn fs::Fs>::set_global(fs.clone(), cx));
    let project =
        project::Project::test(fs.clone() as Arc<dyn fs::Fs>, ["/my-project".as_ref()], cx).await;
    project.update(cx, |p, cx| p.git_scans_complete(cx)).await;

    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar = setup_sidebar(&multi_workspace, cx);

    // Step 1: Create a linked worktree for the main project.
    let worktree_name = "wt-0";
    let worktree_path = "/worktrees/wt-0";

    fs.insert_tree(
        worktree_path,
        serde_json::json!({
            ".git": "gitdir: /my-project/.git/worktrees/wt-0",
            "src": {},
        }),
    )
    .await;
    fs.insert_tree(
        "/my-project/.git/worktrees/wt-0",
        serde_json::json!({
            "commondir": "../../",
            "HEAD": "ref: refs/heads/wt-0",
        }),
    )
    .await;
    fs.add_linked_worktree_for_repo(
        Path::new("/my-project/.git"),
        false,
        git::repository::Worktree {
            path: PathBuf::from(worktree_path),
            ref_name: Some(format!("refs/heads/{}", worktree_name).into()),
            sha: "aaa".into(),
            is_main: false,
            is_bare: false,
        },
    )
    .await;

    let main_workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let main_project = main_workspace.read_with(cx, |ws, _| ws.project().clone());
    main_project
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;
    cx.run_until_parked();

    // Step 2: Open the linked worktree as its own workspace.
    let worktree_project =
        project::Project::test(fs.clone() as Arc<dyn fs::Fs>, [worktree_path.as_ref()], cx).await;
    worktree_project
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;
    let worktree_workspace = multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(worktree_project.clone(), window, cx)
    });
    cx.run_until_parked();

    // Step 3: Add an unrelated project.
    fs.insert_tree(
        "/other-project",
        serde_json::json!({
            ".git": {},
            "src": {},
        }),
    )
    .await;
    let other_project = project::Project::test(
        fs.clone() as Arc<dyn fs::Fs>,
        ["/other-project".as_ref()],
        cx,
    )
    .await;
    other_project
        .update(cx, |p, cx| p.git_scans_complete(cx))
        .await;
    multi_workspace.update_in(cx, |mw, window, cx| {
        mw.test_add_workspace(other_project.clone(), window, cx);
    });
    cx.run_until_parked();

    // Force a full sidebar rebuild with all groups expanded.
    sidebar.update_in(cx, |sidebar, _window, cx| {
        if let Some(mw) = sidebar.multi_workspace.upgrade() {
            mw.update(cx, |mw, _cx| mw.test_expand_all_groups());
        }
        sidebar.update_entries(cx);
    });
    cx.run_until_parked();

    // The linked-worktree workspace must be reachable from at least one
    // sidebar entry — otherwise the user has no way to navigate to it.
    let worktree_ws_id = worktree_workspace.entity_id();
    let (all_ids, reachable_ids) = sidebar.read_with(cx, |sidebar, cx| {
        let mw = multi_workspace.read(cx);

        let all: HashSet<gpui::EntityId> = mw.workspaces().map(|ws| ws.entity_id()).collect();
        let reachable: HashSet<gpui::EntityId> = sidebar
            .contents
            .entries
            .iter()
            .flat_map(|entry| entry.reachable_workspaces(mw, cx))
            .map(|ws| ws.entity_id())
            .collect();
        (all, reachable)
    });

    let unreachable = &all_ids - &reachable_ids;
    eprintln!("{}", visible_entries_as_strings(&sidebar, cx).join("\n"));

    assert!(
        unreachable.is_empty(),
        "workspaces not reachable from any sidebar entry: {:?}\n\
         (linked-worktree workspace id: {:?})",
        unreachable,
        worktree_ws_id,
    );
}
