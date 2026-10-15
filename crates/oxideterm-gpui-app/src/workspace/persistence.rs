use super::*;

mod capture;
mod layout;
mod model;
mod pages;
mod restore;
mod standalone;
mod terminals;
#[cfg(test)]
mod tests;

use model::{SavedTab, SavedTerminal, WorkspaceSnapshot};

const WORKSPACE_SAVE_DELAY: Duration = Duration::from_millis(500);
const MAX_WORKSPACE_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct WorkspacePersistence {
    last_saved: Option<WorkspaceSnapshot>,
    save_task: Option<Task<()>>,
    observation: Option<Subscription>,
    quitting: bool,
    reconnect_nodes: VecDeque<NodeId>,
    reconnect_standalone: VecDeque<String>,
    pending_terminals: HashMap<String, TerminalSessionId>,
    pending_desktops: HashMap<String, TabId>,
    pending_desktop_bounds: HashMap<String, oxideterm_settings::WindowGeometry>,
    preferred_tab: Option<TabId>,
}

fn workspace_snapshot_path() -> PathBuf {
    default_session_tree_path().with_file_name("workspace.json")
}

impl WorkspaceApp {
    pub(in crate::workspace) fn start_workspace_autosave(&mut self, cx: &mut Context<Self>) {
        self.workspace_persistence.observation = Some(cx.observe_self(move |workspace, cx| {
            workspace.remember_workspace_restore_selection(cx);
            workspace.resume_workspace_connections(cx);
            workspace.schedule_workspace_snapshot(cx);
            if !workspace.workspace_persistence.pending_terminals.is_empty()
                || !workspace.workspace_persistence.pending_desktops.is_empty()
            {
                workspace.enqueue_workspace_restore(cx);
            }
        }));
        cx.on_app_quit(|workspace, cx| {
            workspace.prepare_workspace_quit(cx);
            async {}
        })
        .detach();
    }

    fn schedule_workspace_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.workspace_persistence.quitting || self.workspace_persistence.save_task.is_some() {
            return;
        }
        let timer = cx.background_executor().timer(WORKSPACE_SAVE_DELAY);
        self.workspace_persistence.save_task = Some(cx.spawn(async move |workspace, cx| {
            timer.await;
            if let Err(error) = workspace.update(cx, |workspace, cx| {
                workspace.workspace_persistence.save_task = None;
                workspace.flush_workspace_snapshot(cx);
            }) {
                eprintln!("workspace autosave owner was released: {error}");
            }
        }));
    }

    pub(in crate::workspace) fn prepare_workspace_quit(&mut self, cx: &mut App) {
        self.flush_workspace_snapshot(cx);
        self.flush_main_window_state(cx);
        self.workspace_persistence.quitting = true;
    }

    pub(in crate::workspace) fn cancel_workspace_quit(&mut self, cx: &mut Context<Self>) {
        self.workspace_persistence.quitting = false;
        cx.notify();
    }

    pub(in crate::workspace) fn flush_workspace_snapshot(&mut self, cx: &mut App) {
        self.workspace_persistence.save_task = None;
        if self.workspace_persistence.quitting {
            return;
        }
        let snapshot = self.capture_workspace_snapshot(cx);
        if self.workspace_persistence.last_saved.as_ref() == Some(&snapshot) {
            return;
        }
        let result = serde_json::to_vec_pretty(&snapshot)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                oxideterm_atomic_file::durable_write(&workspace_snapshot_path(), &bytes)
                    .map_err(anyhow::Error::from)
            });
        match result {
            Ok(()) => self.workspace_persistence.last_saved = Some(snapshot),
            Err(error) => eprintln!("failed to save workspace snapshot: {error}"),
        }
    }

    fn resume_workspace_connections(&mut self, cx: &mut Context<Self>) {
        if self.workspace_persistence.quitting
            || self.app_lock.locked
            || self
                .workspace_runtime
                .read(cx)
                .has_active_connection_chain()
        {
            return;
        }
        if let Some(node_id) = self.workspace_persistence.reconnect_nodes.pop_front() {
            self.ensure_node_connection_started(&node_id, cx);
            cx.notify();
        }
    }
}
