use super::*;

impl WorkspaceApp {
    pub(super) fn capture_workspace_snapshot(&self, cx: &mut App) -> WorkspaceSnapshot {
        let host = self.tab_host.read(cx);
        let mut terminals = HashMap::new();
        for tab in host.tabs() {
            let mut ids = Vec::new();
            if let Some(root) = &tab.root_pane {
                root.collect_session_ids(&mut ids);
            }
            for session_id in ids {
                let saved = if let Some(instance) = host.local_sessions.get(&session_id) {
                    let cwd = host
                        .terminal_location(session_id)
                        .and_then(|location| host.panes().get(&location.pane_id))
                        .and_then(|pane| pane.read(cx).current_working_directory())
                        .map(PathBuf::from)
                        .or_else(|| instance.cwd.clone());
                    Some(SavedTerminal::Local {
                        shell_id: instance.shell.as_ref().map(|shell| shell.id.clone()),
                        cwd,
                        profile_id: instance.profile_id.clone(),
                    })
                } else if let Some(node_id) = self
                    .workspace_runtime
                    .read(cx)
                    .ssh_terminal_node_id(session_id)
                {
                    Some(SavedTerminal::Ssh { node_id: node_id.0 })
                } else {
                    self.standalone_connections
                        .connection_id_for_surface(
                            standalone_connections::StandaloneConnectionSurface::Terminal(
                                session_id,
                            ),
                        )
                        .or_else(|| {
                            self.workspace_persistence
                                .pending_terminals
                                .iter()
                                .find_map(|(id, slot)| (*slot == session_id).then(|| id.clone()))
                        })
                        .map(|connection_id| SavedTerminal::Standalone { connection_id })
                };
                if let Some(saved) = saved {
                    terminals.insert(session_id, saved);
                }
            }
        }
        let mut tabs = Vec::new();
        let mut detached = Vec::new();
        for tab in host.tabs() {
            let root = tab
                .root_pane
                .as_ref()
                .and_then(|root| restorable_pane(root, &terminals));
            if tab.root_pane.is_some() && root.is_none() {
                continue;
            }
            let node_id = self
                .sftp_tab_nodes
                .get(&tab.id)
                .cloned()
                .or_else(|| self.forwarding.read(cx).node_for_tab(tab.id))
                .or_else(|| self.ide_workspace.read(cx).node_for_tab(tab.id).cloned());
            let standalone_id = self
                .standalone_connections
                .connection_id_for_surface(
                    standalone_connections::StandaloneConnectionSurface::RemoteDesktop(tab.id),
                )
                .or_else(|| {
                    self.workspace_persistence
                        .pending_desktops
                        .iter()
                        .find_map(|(id, slot)| (*slot == tab.id).then(|| id.clone()))
                });
            let pending_bounds = standalone_id.as_ref().and_then(|id| {
                self.workspace_persistence
                    .pending_desktop_bounds
                    .get(id)
                    .copied()
            });
            if let Some(handle) = host.detached_window_handle(tab.id) {
                detached.push((tabs.len(), handle));
            }
            tabs.push(SavedTab {
                id: tab.id,
                kind: tab.kind.clone(),
                title: tab.title.clone(),
                active_pane: tab
                    .active_pane_id
                    .filter(|id| root.as_ref().is_some_and(|root| root.contains_pane(*id)))
                    .or_else(|| root.as_ref().map(PaneNode::first_pane_id)),
                root,
                node_id: node_id.map(|id| id.0),
                standalone_id,
                detached_bounds: pending_bounds,
            });
        }
        let mut connected_nodes = self
            .ssh_nodes
            .iter()
            .filter(|(_, node)| {
                matches!(
                    node.readiness,
                    NodeReadiness::Ready | NodeReadiness::Connecting
                )
            })
            .map(|(id, _)| id.0.clone())
            .collect::<Vec<_>>();
        connected_nodes.extend(
            self.workspace_persistence
                .reconnect_nodes
                .iter()
                .map(|id| id.0.clone()),
        );
        connected_nodes.sort();
        connected_nodes.dedup();
        let active_tab = host.active_tab_id();
        for (index, handle) in detached {
            match handle.update(cx, |_, window, _| {
                root::window_state::geometry_from_window(window)
            }) {
                Ok(bounds) => tabs[index].detached_bounds = bounds,
                Err(error) => eprintln!("failed to capture detached workspace bounds: {error}"),
            }
        }
        WorkspaceSnapshot {
            tabs,
            active_tab,
            terminals,
            connected_nodes,
        }
    }
}

fn restorable_pane(
    node: &PaneNode,
    terminals: &HashMap<TerminalSessionId, SavedTerminal>,
) -> Option<PaneNode> {
    match node {
        PaneNode::Leaf { session_id, .. } => {
            terminals.contains_key(session_id).then(|| node.clone())
        }
        PaneNode::Page { .. } => Some(node.clone()),
        PaneNode::Group {
            id,
            direction,
            children,
        } => {
            let mut children = children
                .iter()
                .filter_map(|child| {
                    restorable_pane(&child.node, terminals)
                        .map(|node| oxideterm_workspace::PaneSplitChild::new(node, child.size))
                })
                .collect::<Vec<_>>();
            match children.len() {
                0 => None,
                1 => Some(children.remove(0).node),
                _ => Some(PaneNode::Group {
                    id: *id,
                    direction: *direction,
                    children,
                }),
            }
        }
    }
}
