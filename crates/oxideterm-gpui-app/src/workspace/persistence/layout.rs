use super::*;

impl WorkspaceApp {
    pub(super) fn restore_workspace_pane(
        &mut self,
        node: &PaneNode,
        terminals: &HashMap<TerminalSessionId, SavedTerminal>,
        tabs: &HashMap<TabId, TabId>,
        panes: &mut HashMap<PaneId, PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneNode> {
        match node {
            PaneNode::Leaf {
                pane_id,
                session_id,
            } => {
                let terminal = terminals.get(session_id)?;
                match self.restore_workspace_terminal(terminal, window, cx) {
                    Ok((restored_pane, restored_session)) => {
                        panes.insert(*pane_id, restored_pane);
                        Some(PaneNode::leaf(restored_pane, restored_session))
                    }
                    Err(error) => {
                        eprintln!("failed to restore workspace terminal: {error}");
                        None
                    }
                }
            }
            PaneNode::Page { pane_id, tab_id } => {
                let tab_id = *tabs.get(tab_id)?;
                let restored_pane = self.alloc_pane_id(cx);
                panes.insert(*pane_id, restored_pane);
                Some(PaneNode::Page {
                    pane_id: restored_pane,
                    tab_id,
                })
            }
            PaneNode::Group {
                direction,
                children,
                ..
            } => {
                let mut restored = children
                    .iter()
                    .filter_map(|child| {
                        self.restore_workspace_pane(&child.node, terminals, tabs, panes, window, cx)
                            .map(|node| oxideterm_workspace::PaneSplitChild::new(node, child.size))
                    })
                    .collect::<Vec<_>>();
                match restored.len() {
                    0 => None,
                    1 => Some(restored.remove(0).node),
                    _ => Some(PaneNode::Group {
                        id: self.alloc_pane_id(cx),
                        direction: *direction,
                        children: restored,
                    }),
                }
            }
        }
    }
}
