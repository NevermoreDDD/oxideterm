use super::*;

fn pending_terminal(
    workspace: &mut WorkspaceApp,
    window: &mut Window,
    cx: &mut Context<WorkspaceApp>,
) -> (TabId, TerminalSessionId, String, TabId) {
    workspace.open_settings_tab(window, cx);
    let preferred = workspace.active_tab_id(cx).unwrap();
    let slot_tab = workspace.alloc_tab_id(cx);
    let slot_pane = workspace.alloc_pane_id(cx);
    let slot_session = workspace.alloc_session_id(cx);
    workspace.insert_tab(
        Tab {
            id: slot_tab,
            kind: TabKind::LocalTerminal,
            title: "Restored Telnet".into(),
            title_source: TabTitleSource::Static,
            root_pane: Some(PaneNode::leaf(slot_pane, slot_session)),
            active_pane_id: Some(slot_pane),
        },
        cx,
    );
    workspace.set_main_window_active_tab(Some(preferred), cx);
    workspace.remember_workspace_restore_selection(cx);
    let actual = workspace
        .create_telnet_terminal_tab_for_connection(
            TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: 0,
                line_endings: Default::default(),
            },
            oxideterm_connections::SavedUpstreamProxyPolicy::Direct,
            None,
            Default::default(),
            "Restored Telnet".into(),
            None,
            window,
            cx,
        )
        .unwrap();
    let connection = workspace
        .standalone_connections
        .connection_id_for_surface(
            standalone_connections::StandaloneConnectionSurface::Terminal(actual),
        )
        .unwrap();
    workspace
        .workspace_persistence
        .pending_terminals
        .insert(connection.clone(), slot_session);
    workspace.remember_workspace_restore_selection(cx);
    (slot_tab, actual, connection, preferred)
}

#[gpui::test]
fn late_standalone_terminal_keeps_its_reserved_tab_and_the_selected_page(cx: &mut TestAppContext) {
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
        let (slot_tab, actual, _, preferred) = pending_terminal(workspace, window, cx);
        let source = workspace
            .tab_host
            .read(cx)
            .terminal_location(actual)
            .unwrap();
        workspace.complete_workspace_standalone_restore(window, cx);
        assert_eq!(workspace.active_tab_id(cx), Some(preferred));
        assert_eq!(
            workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            [preferred, slot_tab]
        );
        assert_eq!(
            workspace.tab_by_id(slot_tab, cx).unwrap().root_pane,
            Some(PaneNode::leaf(source.pane_id, actual))
        );
        assert_eq!(
            workspace.tab_host.read(cx).terminal_location(actual),
            Some(tabs::TerminalLocation {
                tab_id: slot_tab,
                pane_id: source.pane_id
            })
        );
    });
    window_cx.update(|window, _| window.remove_window());
}

#[gpui::test]
fn closing_a_restore_slot_cancels_its_late_connection(cx: &mut TestAppContext) {
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
        let (slot_tab, actual, connection, preferred) = pending_terminal(workspace, window, cx);
        workspace.close_tab_by_id(slot_tab, window, cx);
        workspace.complete_workspace_standalone_restore(window, cx);
        assert_eq!(
            workspace
                .tabs(cx)
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            [preferred]
        );
        assert_eq!(workspace.tab_host.read(cx).terminal_location(actual), None);
        assert_eq!(
            workspace
                .standalone_connections
                .record(&connection)
                .unwrap()
                .readiness,
            ActiveSessionReadiness::Disconnected
        );
        assert!(
            !workspace
                .workspace_persistence
                .pending_terminals
                .contains_key(&connection)
        );
    });
    window_cx.update(|window, _| window.remove_window());
}

#[gpui::test]
fn cancelling_quit_resumes_autosaving_the_current_layout(cx: &mut TestAppContext) {
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
        workspace.open_settings_tab(window, cx);
        workspace.prepare_workspace_quit(cx);
        let saved: WorkspaceSnapshot =
            serde_json::from_slice(&fs::read(workspace_snapshot_path()).unwrap()).unwrap();
        assert_eq!(
            saved
                .tabs
                .iter()
                .map(|tab| tab.kind.clone())
                .collect::<Vec<_>>(),
            [TabKind::Settings]
        );
        workspace.cancel_workspace_quit(cx);
        workspace.close_tab_by_id(workspace.active_tab_id(cx).unwrap(), window, cx);
    });
    window_cx.run_until_parked();
    window_cx.executor().advance_clock(WORKSPACE_SAVE_DELAY);
    window_cx.run_until_parked();
    let saved: WorkspaceSnapshot =
        serde_json::from_slice(&fs::read(workspace_snapshot_path()).unwrap()).unwrap();
    assert_eq!(
        saved
            .tabs
            .iter()
            .map(|tab| tab.kind.clone())
            .collect::<Vec<_>>(),
        Vec::<TabKind>::new()
    );
    window_cx.update(|window, _| window.remove_window());
}
