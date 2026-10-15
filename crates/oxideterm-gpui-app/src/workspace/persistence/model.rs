use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct WorkspaceSnapshot {
    pub(super) tabs: Vec<SavedTab>,
    pub(super) active_tab: Option<TabId>,
    pub(super) terminals: HashMap<TerminalSessionId, SavedTerminal>,
    pub(super) connected_nodes: Vec<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedTab {
    pub(super) id: TabId,
    pub(super) kind: TabKind,
    pub(super) title: String,
    pub(super) root: Option<PaneNode>,
    pub(super) active_pane: Option<PaneId>,
    pub(super) node_id: Option<String>,
    pub(super) standalone_id: Option<String>,
    pub(super) detached_bounds: Option<oxideterm_settings::WindowGeometry>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum SavedTerminal {
    Local {
        shell_id: Option<String>,
        cwd: Option<PathBuf>,
        profile_id: Option<String>,
    },
    Ssh {
        node_id: String,
    },
    Standalone {
        connection_id: String,
    },
}

impl WorkspaceSnapshot {
    pub(super) fn validate(&self) -> Result<()> {
        let mut tabs = HashSet::new();
        let mut panes = HashSet::new();
        let mut sessions = HashSet::new();
        let mut pages = HashSet::new();
        for tab in &self.tabs {
            anyhow::ensure!(tabs.insert(tab.id), "duplicate workspace tab");
            if let Some(root) = &tab.root {
                anyhow::ensure!(
                    root.pane_count() <= MAX_PANES_PER_TAB,
                    "too many workspace panes"
                );
                validate_pane(root, &mut panes, &mut sessions, &mut pages)?;
                anyhow::ensure!(
                    tab.active_pane.is_none_or(|pane| root.contains_pane(pane)),
                    "invalid active workspace pane"
                );
            }
        }
        anyhow::ensure!(pages.is_subset(&tabs), "missing workspace page");
        anyhow::ensure!(
            sessions.iter().all(|id| self.terminals.contains_key(id)),
            "missing workspace terminal"
        );
        for tab in &self.tabs {
            if let Some(root) = &tab.root {
                let mut embedded = Vec::new();
                root.collect_page_ids(&mut embedded);
                anyhow::ensure!(
                    embedded.iter().all(|id| {
                        self.tabs
                            .iter()
                            .any(|page| page.id == *id && page.root.is_none())
                    }),
                    "recursive workspace page"
                );
            }
        }
        Ok(())
    }
}

fn validate_pane(
    node: &PaneNode,
    panes: &mut HashSet<PaneId>,
    sessions: &mut HashSet<TerminalSessionId>,
    pages: &mut HashSet<TabId>,
) -> Result<()> {
    match node {
        PaneNode::Leaf {
            pane_id,
            session_id,
        } => {
            anyhow::ensure!(panes.insert(*pane_id), "duplicate workspace pane");
            anyhow::ensure!(sessions.insert(*session_id), "duplicate workspace terminal");
        }
        PaneNode::Page { pane_id, tab_id } => {
            anyhow::ensure!(panes.insert(*pane_id), "duplicate workspace pane");
            anyhow::ensure!(pages.insert(*tab_id), "duplicate workspace page");
        }
        PaneNode::Group { id, children, .. } => {
            anyhow::ensure!(panes.insert(*id), "duplicate workspace split");
            anyhow::ensure!(children.len() >= 2, "empty workspace split");
            for child in children {
                anyhow::ensure!(
                    child.size.is_finite() && child.size > 0.0,
                    "invalid workspace split size"
                );
                validate_pane(&child.node, panes, sessions, pages)?;
            }
        }
    }
    Ok(())
}
