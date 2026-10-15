use super::*;

impl WorkspaceApp {
    pub(in crate::workspace) fn restore_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = workspace_snapshot_path();
        let result = (|| -> Result<Option<WorkspaceSnapshot>> {
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            anyhow::ensure!(
                metadata.len() <= MAX_WORKSPACE_SNAPSHOT_BYTES,
                "workspace snapshot exceeds size limit"
            );
            let bytes = fs::read(path)?;
            let snapshot: WorkspaceSnapshot = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid workspace snapshot"))?;
            snapshot.validate()?;
            Ok(Some(snapshot))
        })();
        match result {
            Ok(Some(snapshot)) => self.apply_workspace_snapshot(snapshot, window, cx),
            Ok(None) => {}
            Err(error) => eprintln!("failed to load workspace snapshot: {error}"),
        }
    }

    pub(super) fn apply_workspace_snapshot(
        &mut self,
        snapshot: WorkspaceSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut tab_ids = HashMap::new();
        let mut bases = HashMap::new();
        for saved in &snapshot.tabs {
            match self.restore_workspace_page(saved, window, cx) {
                Ok(Some(tab)) => {
                    if tab_ids.values().any(|id| *id == tab.id) {
                        continue;
                    }
                    tab_ids.insert(saved.id, tab.id);
                    bases.insert(saved.id, tab);
                }
                Ok(None) => {}
                Err(error) => eprintln!("failed to restore workspace page: {error}"),
            }
        }
        let mut restored = Vec::new();
        let mut detached = Vec::new();
        for saved in &snapshot.tabs {
            let Some(mut tab) = bases.remove(&saved.id) else {
                continue;
            };
            let mut pane_ids = HashMap::new();
            tab.root_pane = saved.root.as_ref().and_then(|root| {
                self.restore_workspace_pane(
                    root,
                    &snapshot.terminals,
                    &tab_ids,
                    &mut pane_ids,
                    window,
                    cx,
                )
            });
            if saved.root.is_some() && tab.root_pane.is_none() {
                continue;
            }
            tab.active_pane_id = saved
                .active_pane
                .and_then(|id| pane_ids.get(&id).copied())
                .or_else(|| tab.root_pane.as_ref().map(PaneNode::first_pane_id));
            if let Some(bounds) = saved
                .detached_bounds
                .filter(|_| saved.kind != TabKind::RemoteDesktop)
            {
                detached.push((tab.id, bounds));
            }
            self.register_tab_surface(&tab, cx);
            restored.push(tab);
        }
        self.tab_host
            .update(cx, |host, _| host.restore_tab_layouts(restored));
        let active = snapshot
            .active_tab
            .and_then(|id| tab_ids.get(&id).copied())
            .filter(|id| self.tab_by_id(*id, cx).is_some())
            .or_else(|| self.tabs(cx).first().map(|tab| tab.id));
        self.set_main_window_active_tab(active, cx);
        self.workspace_persistence.preferred_tab = active;
        self.sync_active_tab_surface(cx);
        self.workspace_persistence.reconnect_nodes = snapshot
            .connected_nodes
            .into_iter()
            .map(NodeId::new)
            .filter(|id| self.node_router.contains_node(id))
            .collect();
        self.workspace_persistence.last_saved = None;
        self.needs_active_pane_focus = true;
        cx.defer_in(window, move |workspace, window, cx| {
            for (tab_id, geometry) in detached {
                let state = oxideterm_settings::WindowUiState {
                    normal_bounds: Some(geometry),
                    ..Default::default()
                };
                let bounds =
                    crate::window_placement::initial_window_bounds(cx, &state).get_bounds();
                workspace.detach_tab_to_window_at_bounds(tab_id, None, bounds, window, cx);
            }
            workspace.complete_workspace_standalone_restore(window, cx);
            workspace.resume_workspace_connections(cx);
            workspace.focus_active_pane(window, cx);
            cx.notify();
        });
    }
}
