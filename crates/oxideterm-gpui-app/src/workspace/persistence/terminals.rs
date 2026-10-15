use super::*;

impl WorkspaceApp {
    pub(super) fn restore_workspace_terminal(
        &mut self,
        terminal: &SavedTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(PaneId, TerminalSessionId)> {
        match terminal {
            SavedTerminal::Local {
                shell_id,
                cwd,
                profile_id,
            } => {
                let mut config = self.local_terminal_config();
                if let Some(shell_id) = shell_id {
                    config.shell = Some(
                        self.effective_local_shells_for_settings(self.settings_store.settings())
                            .into_iter()
                            .find(|shell| shell.id == *shell_id)
                            .ok_or_else(|| anyhow::anyhow!("saved local shell is unavailable"))?,
                    );
                }
                config.cwd = cwd.clone();
                let title = config
                    .shell
                    .as_ref()
                    .map(|shell| shell.label.clone())
                    .unwrap_or_else(|| self.local_terminal_tab_title());
                let (session_id, _) =
                    self.create_local_terminal_tab_with_owned_session(config, title, window, cx)?;
                let location = self
                    .tab_host
                    .read(cx)
                    .terminal_location(session_id)
                    .ok_or_else(|| anyhow::anyhow!("restored local terminal has no location"))?;
                let index = self
                    .tab_index_by_id(location.tab_id, cx)
                    .ok_or_else(|| anyhow::anyhow!("restored local terminal has no tab"))?;
                self.tab_host.update(cx, |host, _| {
                    if let Some(instance) = host.local_sessions.get_mut(&session_id) {
                        instance.profile_id = profile_id.clone();
                    }
                    host.remove_tab_at(index);
                });
                self.ai_runtime_context
                    .update(cx, |runtime, _| runtime.revoke_app_surface(location.tab_id));
                Ok((location.pane_id, session_id))
            }
            SavedTerminal::Ssh { node_id } => {
                let node_id = NodeId::new(node_id);
                let metadata = self
                    .node_router
                    .node_metadata(&node_id)
                    .ok_or_else(|| anyhow::anyhow!("saved SSH node is unavailable"))?;
                let pane_id = self.alloc_pane_id(cx);
                let session_id = self.alloc_session_id(cx);
                let config =
                    SshSessionConfig::disconnected(metadata.host, metadata.port, metadata.username)
                        .with_runtime(self.forwarding_runtime.clone());
                let overrides = self.terminal_preference_overrides_for_ssh_node(&node_id);
                let mut preferences =
                    self.prepare_terminal_preferences_for_tab_kind(&TabKind::SshTerminal, cx);
                overrides.apply_to(&mut preferences);
                let session = TerminalPane::ssh_shared_session(config, &preferences);
                let pane = cx.new(|cx| {
                    #[expect(clippy::expect_used, reason = "pane initialization from an existing terminal session is infallible")]
                    TerminalPane::from_shared_session(session.clone(), preferences, window, cx)
                        .expect("existing terminal session initializes a pane")
                        .with_preference_overrides(overrides)
                });
                self.register_existing_ssh_terminal_session(&node_id, session_id, cx)?;
                self.register_terminal_endpoint_session(&node_id, session_id, session, cx);
                self.register_terminal_pane(pane_id, session_id, pane, window, cx);
                self.workspace_runtime.update(cx, |runtime, cx| {
                    runtime.queue_ssh_terminal_open(
                        runtime_entity::PendingSshTerminalOpen {
                            node_id,
                            reconnect_session_id: Some(session_id),
                            post_connect_command: None,
                            mark_used_connection_id: None,
                            save_after_open: None,
                            cleanup_node_id: None,
                            title: String::new(),
                        },
                        cx,
                    );
                });
                Ok((pane_id, session_id))
            }
            SavedTerminal::Standalone { connection_id } => {
                anyhow::ensure!(
                    self.standalone_connections.record(connection_id).is_some(),
                    "saved connection is unavailable"
                );
                let pane_id = self.alloc_pane_id(cx);
                let session_id = self.alloc_session_id(cx);
                self.workspace_persistence
                    .pending_terminals
                    .insert(connection_id.clone(), session_id);
                self.workspace_persistence
                    .reconnect_standalone
                    .push_back(connection_id.clone());
                Ok((pane_id, session_id))
            }
        }
    }
}
