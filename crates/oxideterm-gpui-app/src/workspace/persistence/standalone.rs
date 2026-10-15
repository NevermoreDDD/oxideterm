use super::*;

impl WorkspaceApp {
    pub(super) fn remember_workspace_restore_selection(&mut self, cx: &App) {
        let active = self.active_tab_id(cx);
        let generated = self
            .workspace_persistence
            .pending_terminals
            .keys()
            .chain(self.workspace_persistence.pending_desktops.keys())
            .filter_map(|id| {
                self.standalone_connections
                    .record(id)
                    .and_then(|record| record.surface)
            })
            .any(|surface| match surface {
                standalone_connections::StandaloneConnectionSurface::Terminal(id) => self
                    .tab_host
                    .read(cx)
                    .terminal_location(id)
                    .is_some_and(|location| Some(location.tab_id) == active),
                standalone_connections::StandaloneConnectionSurface::RemoteDesktop(id) => {
                    Some(id) == active
                }
            });
        if !generated {
            self.workspace_persistence.preferred_tab = active;
        }
    }

    pub(in crate::workspace) fn complete_workspace_standalone_restore(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_persistence.quitting || self.app_lock.locked {
            return;
        }
        let terminals = self.workspace_persistence.pending_terminals.clone();
        for (connection_id, slot) in terminals {
            let slot_exists = self.tabs(cx).iter().any(|tab| {
                tab.root_pane
                    .as_ref()
                    .is_some_and(|root| root.pane_id_for_session(slot).is_some())
            });
            if !slot_exists {
                self.workspace_persistence
                    .pending_terminals
                    .remove(&connection_id);
                self.workspace_persistence
                    .reconnect_standalone
                    .retain(|id| *id != connection_id);
                self.disconnect_standalone_connection(&connection_id, window, cx);
                continue;
            }
            let surface = self
                .standalone_connections
                .record(&connection_id)
                .and_then(|record| record.surface);
            if let Some(standalone_connections::StandaloneConnectionSurface::Terminal(actual)) =
                surface
            {
                let source = self
                    .tab_host
                    .read(cx)
                    .terminal_location(actual)
                    .map(|location| location.tab_id);
                let target = self
                    .tab_host
                    .update(cx, |host, _| host.install_restored_terminal(slot, actual));
                if target.is_some() {
                    if let Some(source) = source {
                        self.ai_runtime_context
                            .update(cx, |runtime, _| runtime.revoke_app_surface(source));
                    }
                    let preferred = self
                        .workspace_persistence
                        .preferred_tab
                        .filter(|id| self.tab_by_id(*id, cx).is_some());
                    self.set_main_window_active_tab(preferred, cx);
                } else {
                    self.disconnect_standalone_connection(&connection_id, window, cx);
                }
                self.workspace_persistence
                    .pending_terminals
                    .remove(&connection_id);
                cx.notify();
            }
        }
        let desktops = self.workspace_persistence.pending_desktops.clone();
        for (connection_id, slot) in desktops {
            if self.tab_by_id(slot, cx).is_none() {
                self.workspace_persistence
                    .pending_desktops
                    .remove(&connection_id);
                self.workspace_persistence
                    .pending_desktop_bounds
                    .remove(&connection_id);
                self.workspace_persistence
                    .reconnect_standalone
                    .retain(|id| *id != connection_id);
                self.disconnect_standalone_connection(&connection_id, window, cx);
                continue;
            }
            if let Some(standalone_connections::StandaloneConnectionSurface::RemoteDesktop(
                actual,
            )) = self
                .standalone_connections
                .record(&connection_id)
                .and_then(|record| record.surface)
            {
                if self
                    .tab_host
                    .update(cx, |host, _| host.install_restored_desktop(slot, actual))
                {
                    self.ai_runtime_context
                        .update(cx, |runtime, _| runtime.revoke_app_surface(slot));
                    let preferred = self
                        .workspace_persistence
                        .preferred_tab
                        .map(|id| if id == slot { actual } else { id })
                        .filter(|id| self.tab_by_id(*id, cx).is_some());
                    self.workspace_persistence.preferred_tab = preferred;
                    self.set_main_window_active_tab(preferred, cx);
                    if let Some(geometry) = self
                        .workspace_persistence
                        .pending_desktop_bounds
                        .remove(&connection_id)
                    {
                        let state = oxideterm_settings::WindowUiState {
                            normal_bounds: Some(geometry),
                            ..Default::default()
                        };
                        let bounds =
                            crate::window_placement::initial_window_bounds(cx, &state).get_bounds();
                        self.detach_tab_to_window_at_bounds(actual, None, bounds, window, cx);
                    }
                }
                self.workspace_persistence
                    .pending_desktops
                    .remove(&connection_id);
                cx.notify();
            }
        }
        let connecting = self
            .standalone_connections
            .records()
            .iter()
            .any(|record| record.readiness == ActiveSessionReadiness::Connecting);
        if !connecting && let Some(id) = self.workspace_persistence.reconnect_standalone.pop_front()
        {
            self.reconnect_standalone_connection(&id, window, cx);
            cx.notify();
        }
    }
}
