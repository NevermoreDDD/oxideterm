use super::*;

impl WorkspaceTabHostEntity {
    pub(in crate::workspace) fn restore_tab_layouts(&mut self, tabs: Vec<Tab>) {
        self.tabs = tabs;
        self.embedded_pages.clear();
        self.terminal_locations.clear();
        for tab in &self.tabs {
            if let Some(root) = &tab.root_pane {
                let mut pages = Vec::new();
                root.collect_page_ids(&mut pages);
                for page in pages {
                    self.embedded_pages.insert(page, tab.id);
                }
                let mut sessions = Vec::new();
                root.collect_session_ids(&mut sessions);
                for session_id in sessions {
                    if let Some(pane_id) = root
                        .pane_id_for_session(session_id)
                        .filter(|pane_id| self.panes.contains_key(pane_id))
                    {
                        self.terminal_locations.insert(
                            session_id,
                            TerminalLocation {
                                tab_id: tab.id,
                                pane_id,
                            },
                        );
                    }
                }
            }
        }
        self.active_tab_index_cache.set(None);
    }

    pub(in crate::workspace) fn install_restored_terminal(
        &mut self,
        slot: TerminalSessionId,
        session_id: TerminalSessionId,
    ) -> Option<TabId> {
        let location = self.terminal_locations.get(&session_id).copied()?;
        let mut target = None;
        for tab in &mut self.tabs {
            if let Some(root) = &mut tab.root_pane {
                if let Some(old_pane) = root.replace_session(slot, location.pane_id, session_id) {
                    if tab.active_pane_id == Some(old_pane) {
                        tab.active_pane_id = Some(location.pane_id);
                    }
                    target = Some(tab.id);
                    break;
                }
            }
        }
        let target = target?;
        let index = self.tab_index_by_id(location.tab_id)?;
        self.remove_tab_at(index);
        self.terminal_locations.remove(&slot);
        self.terminal_locations.insert(
            session_id,
            TerminalLocation {
                tab_id: target,
                pane_id: location.pane_id,
            },
        );
        let detached = self.detached_window_handle(target);
        if let Some(affinity) = self.pane_window_affinities.get_mut(&location.pane_id) {
            affinity.current = detached.unwrap_or(affinity.home);
        }
        Some(target)
    }

    pub(in crate::workspace) fn install_restored_desktop(
        &mut self,
        slot: TabId,
        actual: TabId,
    ) -> bool {
        let Some(source) = self.tab_index_by_id(actual) else {
            return false;
        };
        let tab = self.tabs.remove(source);
        let Some(target) = self.tab_index_by_id(slot) else {
            self.tabs.insert(source, tab);
            return false;
        };
        self.tabs[target] = tab;
        for tab in &mut self.tabs {
            if let Some(root) = &mut tab.root_pane {
                replace_page(root, slot, actual);
            }
        }
        if let Some(container) = self.embedded_pages.remove(&slot) {
            self.embedded_pages.insert(actual, container);
        }
        self.active_tab_index_cache.set(None);
        true
    }
}

fn replace_page(node: &mut PaneNode, slot: TabId, actual: TabId) {
    match node {
        PaneNode::Page { tab_id, .. } if *tab_id == slot => *tab_id = actual,
        PaneNode::Group { children, .. } => {
            for child in children {
                replace_page(&mut child.node, slot, actual);
            }
        }
        PaneNode::Page { .. } | PaneNode::Leaf { .. } => {}
    }
}
