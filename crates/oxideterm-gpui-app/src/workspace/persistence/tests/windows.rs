use super::*;

#[gpui::test]
fn detached_page_restores_window_placement_and_ownership_after_quit(cx: &mut TestAppContext) {
    if !isolated_fixture(cx) {
        return;
    }
    configure_fixture();
    cx.executor().allow_parking();
    {
        let (shell, window_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
            WorkspaceWindowShell::new(workspace, window, cx)
        });
        let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
        let detached = workspace.update_in(window_cx, |workspace, window, cx| {
            workspace.open_settings_tab(window, cx);
            let id = workspace.active_tab_id(cx).unwrap();
            let bounds = Bounds::new(
                gpui::point(px(80.0), px(100.0)),
                gpui::size(px(960.0), px(640.0)),
            );
            workspace.detach_tab_to_window_at_bounds(id, None, bounds, window, cx);
            let detached = workspace
                .tab_host
                .read(cx)
                .detached_window_handle(id)
                .unwrap();
            workspace.prepare_workspace_quit(cx);
            detached
        });
        let saved: WorkspaceSnapshot =
            serde_json::from_slice(&fs::read(workspace_snapshot_path()).unwrap()).unwrap();
        let placement = saved.tabs[0].detached_bounds.unwrap();
        assert_eq!(
            (placement.x, placement.y, placement.width, placement.height),
            (80, 100, 960, 640)
        );
        window_cx.update(|window, cx| {
            detached
                .update(cx, |_, detached, _| detached.remove_window())
                .unwrap();
            window.remove_window();
        });
        drop(workspace);
        drop(shell);
    }
    cx.run_until_parked();
    let (shell, window_cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| WorkspaceApp::new(window, cx, None, None).unwrap());
        WorkspaceWindowShell::new(workspace, window, cx)
    });
    window_cx.run_until_parked();
    let workspace = shell.read_with(window_cx, |shell, _| shell.session_entity());
    let detached = workspace.read_with(window_cx, |workspace, cx| {
        let page = workspace
            .tabs(cx)
            .iter()
            .find(|tab| tab.kind == TabKind::Settings)
            .unwrap();
        assert_eq!(workspace.active_tab_id(cx), None);
        assert!(workspace.tab_host.read(cx).is_detached(page.id));
        let handle = workspace
            .tab_host
            .read(cx)
            .detached_window_handle(page.id)
            .unwrap();
        assert_eq!(
            workspace
                .window_registry
                .handle_for_role(window_registry::WindowRole::Detached { tab_id: page.id }),
            Some(handle)
        );
        handle
    });
    window_cx.update(|_, cx| {
        detached
            .update(cx, |_, window, _| {
                let bounds = root::window_state::geometry_from_window(window).unwrap();
                assert_eq!(
                    (bounds.x, bounds.y, bounds.width, bounds.height),
                    (80, 100, 960, 640)
                );
            })
            .unwrap();
    });
    workspace.update(window_cx, |workspace, cx| {
        workspace.prepare_workspace_quit(cx)
    });
    window_cx.update(|window, cx| {
        detached
            .update(cx, |_, detached, _| detached.remove_window())
            .unwrap();
        window.remove_window();
    });
}
