use super::*;
use gpui::TestAppContext;

mod standalone;
mod windows;

fn isolated_fixture(cx: &TestAppContext) -> bool {
    const FIXTURE_ENV: &str = "OXIDETERM_WORKSPACE_RESTORE_TEST_DIR";
    if let Some(directory) = std::env::var_os(FIXTURE_ENV) {
        assert!(default_settings_path().starts_with(PathBuf::from(directory)));
        return true;
    }
    let executable = std::env::current_exe().unwrap();
    let directory = tempfile::tempdir_in(executable.parent().unwrap()).unwrap();
    let child = directory.path().join(executable.file_name().unwrap());
    fs::hard_link(&executable, &child).unwrap();
    fs::write(directory.path().join("portable"), []).unwrap();
    let output = std::process::Command::new(child)
        .arg(cx.test_function_name().unwrap())
        .arg("--nocapture")
        .env(FIXTURE_ENV, directory.path())
        .env_remove("APPIMAGE")
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn configure_fixture() {
    let mut store = SettingsStore::load_default().unwrap();
    store.settings_mut().ssh_config.auto_load_hosts = false;
    store.settings_mut().onboarding_completed = true;
    store.save().unwrap();
}

#[gpui::test]
fn autosave_restores_split_layout_focus_and_closed_tabs_after_reopening(cx: &mut TestAppContext) {
    if !isolated_fixture(cx) {
        return;
    }
    configure_fixture();
    cx.executor().allow_parking();
    let directory = tempfile::tempdir().unwrap();
    let work_a = directory.path().canonicalize().unwrap().join("alpha");
    let work_b = directory.path().canonicalize().unwrap().join("beta");
    fs::create_dir(&work_a).unwrap();
    fs::create_dir(&work_b).unwrap();
    {
        let (shell, window_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
            WorkspaceWindowShell::new(workspace, window, cx)
        });
        let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
        workspace.update_in(window_cx, |workspace, window, cx| {
            for (title, cwd) in [("Alpha", work_a.clone()), ("Beta", work_b.clone())] {
                let config = LocalPtyConfig {
                    cwd: Some(cwd),
                    load_profile: false,
                    ..Default::default()
                };
                workspace
                    .create_local_terminal_tab_with_config(config, title.into(), window, cx)
                    .unwrap();
            }
            let ids = workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>();
            workspace.open_settings_tab(window, cx);
            let closed = workspace.active_tab_id(cx).unwrap();
            workspace.close_tab_by_id(closed, window, cx);
            assert!(workspace.combine_tabs_at(
                ids[1],
                ids[0],
                None,
                SplitDirection::Horizontal,
                false,
                window,
                cx
            ));
            let tab = workspace.active_tab(cx).unwrap().clone();
            let PaneNode::Group { id, children, .. } = tab.root_pane.as_ref().unwrap() else {
                panic!("split");
            };
            let active = children[0].node.first_pane_id();
            workspace.tab_host.update(cx, |host, _| {
                host.update_group_sizes(Some(tab.id), *id, &[30.0, 70.0]);
                host.set_active_pane(Some(tab.id), active);
            });
            cx.notify();
        });
        window_cx.run_until_parked();
        window_cx.executor().advance_clock(WORKSPACE_SAVE_DELAY);
        window_cx.run_until_parked();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(workspace_snapshot_path()).unwrap()).unwrap();
        assert_eq!(saved["tabs"][0]["kind"], "Workspace");
        assert_eq!(saved["tabs"][0]["root"]["Group"]["direction"], "Horizontal");
        assert_eq!(saved["tabs"].as_array().unwrap().len(), 1);
        window_cx.update(|window, _| window.remove_window());
        drop(workspace);
        drop(shell);
    }
    cx.run_until_parked();
    let (shell, window_cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
        WorkspaceWindowShell::new(workspace, window, cx)
    });
    let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
    workspace.read_with(window_cx, |workspace, cx| {
        let tab = workspace.active_tab(cx).unwrap();
        let PaneNode::Group {
            direction,
            children,
            ..
        } = tab.root_pane.as_ref().unwrap()
        else {
            panic!("restored split");
        };
        assert_eq!(*direction, SplitDirection::Horizontal);
        assert!((children[0].size - 30.0).abs() < 0.001);
        assert!((children[1].size - 70.0).abs() < 0.001);
        assert_eq!(tab.active_pane_id, Some(children[0].node.first_pane_id()));
        let host = workspace.tab_host.read(cx);
        let directories = children
            .iter()
            .map(|child| {
                let PaneNode::Leaf { session_id, .. } = child.node else {
                    panic!("terminal");
                };
                host.local_sessions[&session_id].cwd.clone().unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(directories, [work_a, work_b]);
        assert_eq!(
            workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.kind.clone())
                .collect::<Vec<_>>(),
            [TabKind::Workspace]
        );
    });
    window_cx.update(|window, _| window.remove_window());
}

#[gpui::test]
fn workspace_snapshot_excludes_runtime_secrets_and_keeps_ssh_identity(cx: &mut TestAppContext) {
    if !isolated_fixture(cx) {
        return;
    }
    configure_fixture();
    cx.executor().allow_parking();
    let (shell, window_cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
        WorkspaceWindowShell::new(workspace, window, cx)
    });
    let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
    workspace.update_in(window_cx, |workspace, window, cx| {
        let config = SshConfig { post_connect_command: Some("synthetic-command-secret".into()),
            ..SshConfig::password("fixture.invalid", 22, "fixture", "synthetic-auth-secret") };
        let node_id = workspace.materialize_ssh_root_node(config, "Fixture".into(), None);
        let (pane_id, session_id) = workspace.restore_workspace_terminal(&SavedTerminal::Ssh { node_id: node_id.0.clone() }, window, cx).unwrap();
        let id = workspace.alloc_tab_id(cx);
        workspace.insert_tab(Tab { id, kind: TabKind::SshTerminal, title: "Fixture".into(),
            title_source: TabTitleSource::Static, root_pane: Some(PaneNode::leaf(pane_id, session_id)), active_pane_id: Some(pane_id) }, cx);
        workspace.bind_terminal_location(id, pane_id, session_id, cx);
        let mut config = LocalPtyConfig { load_profile: false, ..Default::default() };
        config.env.insert("WORKSPACE_SECRET".into(), "synthetic-environment-secret".into());
        workspace.create_local_terminal_tab_with_config(config, "Local".into(), window, cx).unwrap();
        workspace.flush_workspace_snapshot(cx);
        let saved = fs::read_to_string(workspace_snapshot_path()).unwrap();
        for secret in ["synthetic-auth-secret", "synthetic-command-secret", "synthetic-environment-secret"] {
            assert!(!saved.contains(secret), "secret crossed workspace persistence boundary");
        }
        let snapshot: WorkspaceSnapshot = serde_json::from_str(&saved).unwrap();
        assert!(matches!(snapshot.terminals.get(&session_id), Some(SavedTerminal::Ssh { node_id: saved_id }) if *saved_id == node_id.0));
        snapshot.validate().unwrap();
    });
    window_cx.update(|window, _| window.remove_window());
}

#[gpui::test]
fn restoring_an_ide_page_preserves_its_node_and_active_tab(cx: &mut TestAppContext) {
    if !isolated_fixture(cx) {
        return;
    }
    configure_fixture();
    cx.executor().allow_parking();
    let (shell, window_cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
        WorkspaceWindowShell::new(workspace, window, cx)
    });
    let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
    workspace.update_in(window_cx, |workspace, window, cx| {
        let config = SshConfig::password("fixture.invalid", 22, "fixture", "fixture-password");
        let node_id = workspace.materialize_ssh_root_node(config, "Fixture".into(), None);
        let ide_tab = TabId(2);
        let snapshot = WorkspaceSnapshot {
            tabs: vec![
                SavedTab {
                    id: TabId(1),
                    kind: TabKind::Settings,
                    title: "Settings".into(),
                    root: None,
                    active_pane: None,
                    node_id: None,
                    standalone_id: None,
                    detached_bounds: None,
                },
                SavedTab {
                    id: ide_tab,
                    kind: TabKind::Ide,
                    title: "IDE".into(),
                    root: None,
                    active_pane: None,
                    node_id: Some(node_id.0.clone()),
                    standalone_id: None,
                    detached_bounds: None,
                },
            ],
            active_tab: Some(ide_tab),
            terminals: HashMap::new(),
            connected_nodes: Vec::new(),
        };
        workspace.apply_workspace_snapshot(snapshot, window, cx);
        assert_eq!(
            workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.kind.clone())
                .collect::<Vec<_>>(),
            [TabKind::Settings, TabKind::Ide]
        );
        let active = workspace.active_tab(cx).unwrap();
        assert_eq!(active.kind, TabKind::Ide);
        assert_eq!(
            workspace.ide_workspace.read(cx).node_for_tab(active.id),
            Some(&node_id)
        );
    });
    window_cx.update(|window, _| window.remove_window());
}

#[gpui::test]
fn corrupt_workspace_snapshot_opens_an_empty_workspace(cx: &mut TestAppContext) {
    if !isolated_fixture(cx) {
        return;
    }
    configure_fixture();
    fs::write(workspace_snapshot_path(), b"{incomplete").unwrap();
    cx.executor().allow_parking();
    let (shell, window_cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
        WorkspaceWindowShell::new(workspace, window, cx)
    });
    let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
    workspace.read_with(window_cx, |workspace, cx| {
        assert_eq!(
            workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.kind.clone())
                .collect::<Vec<_>>(),
            Vec::<TabKind>::new()
        );
        assert_eq!(workspace.active_tab_id(cx), None);
    });
    window_cx.update(|window, _| window.remove_window());
}
