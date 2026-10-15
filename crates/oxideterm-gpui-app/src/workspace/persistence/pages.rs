use super::*;

impl WorkspaceApp {
    pub(super) fn restore_workspace_page(
        &mut self,
        saved: &SavedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Tab>> {
        let node_id = saved
            .node_id
            .as_ref()
            .map(NodeId::new)
            .filter(|id| self.node_router.contains_node(id));
        match &saved.kind {
            TabKind::Workspace
            | TabKind::LocalTerminal
            | TabKind::SshTerminal
            | TabKind::MoshTerminal => {
                return Ok(Some(Tab {
                    id: self.alloc_tab_id(cx),
                    kind: saved.kind.clone(),
                    title: saved.title.clone(),
                    title_source: TabTitleSource::Static,
                    root_pane: None,
                    active_pane_id: None,
                }));
            }
            TabKind::Settings => self.open_settings_tab(window, cx),
            TabKind::FileManager => self.open_file_manager_tab(window, cx),
            TabKind::Runtime | TabKind::ConnectionPool => {
                self.open_connection_runtime_tab(ConnectionRuntimeSection::Overview, window, cx)
            }
            TabKind::Topology => self.open_topology_tab(window, cx),
            TabKind::NotificationCenter => self.open_notification_center_tab(window, cx),
            TabKind::SessionManager => self.open_session_manager_tab(window, cx),
            TabKind::PluginManager => self.open_plugin_manager_tab(window, cx),
            TabKind::CloudSync => self.open_cloud_sync_tab(window, cx),
            TabKind::Knowledge => self.open_knowledge_workspace_tab(window, cx),
            TabKind::Graphics => self.open_graphics_tab(window, cx),
            TabKind::Sftp => {
                let Some(node_id) = node_id else {
                    return Ok(None);
                };
                self.open_sftp_tab_surface(node_id, None, cx);
            }
            TabKind::Forwards => {
                let Some(node_id) = node_id else {
                    return Ok(None);
                };
                self.open_forwards_tab(node_id, window, cx);
            }
            TabKind::Ide => {
                let Some(node_id) = node_id else {
                    return Ok(None);
                };
                self.open_ide_folder_picker_tab(node_id, cx);
            }
            TabKind::RemoteDesktop => {
                let Some(id) = saved
                    .standalone_id
                    .as_ref()
                    .filter(|id| self.standalone_connections.record(id).is_some())
                else {
                    return Ok(None);
                };
                let tab_id = self.alloc_tab_id(cx);
                self.workspace_persistence
                    .pending_desktops
                    .insert(id.clone(), tab_id);
                if let Some(bounds) = saved.detached_bounds {
                    self.workspace_persistence
                        .pending_desktop_bounds
                        .insert(id.clone(), bounds);
                }
                self.workspace_persistence
                    .reconnect_standalone
                    .push_back(id.clone());
                return Ok(Some(Tab {
                    id: tab_id,
                    kind: saved.kind.clone(),
                    title: saved.title.clone(),
                    title_source: TabTitleSource::Static,
                    root_pane: None,
                    active_pane_id: None,
                }));
            }
            TabKind::Plugin { plugin_id, tab_id } => self
                .open_native_plugin_tab(plugin_id, tab_id, cx)
                .map_err(|_| anyhow::anyhow!("saved plugin tab is unavailable"))?,
        }
        Ok(self.active_tab(cx).cloned())
    }
}
