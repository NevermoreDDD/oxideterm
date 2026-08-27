use super::*;

impl TerminalPane {
    pub fn reconnect_telnet(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session_kind != TerminalSessionKind::Telnet || self.lifecycle().is_running() {
            return false;
        }
        let Some(config) = self.telnet_reconnect_config.clone() else {
            return false;
        };

        let resize = self
            .last_pty_resize
            .unwrap_or((DEFAULT_COLS, DEFAULT_ROWS, 0, 0));
        self.terminal.lock().shutdown();

        // One-shot login credentials are deliberately not retained after the
        // initial connection, so reconnecting preserves only the endpoint.
        let mut terminal = TerminalSession::telnet_with_login_and_encoding(
            config,
            None,
            resize.0,
            resize.1,
            graphics_options_from_preferences(&self.preferences),
            self.preferences.terminal_encoding,
            self.preferences.scrollback_lines,
        );
        if resize.2 > 0 && resize.3 > 0 {
            let _ = terminal.resize_with_cell_size(resize.0, resize.1, resize.2, resize.3);
        }
        let _ = terminal.set_focused(self.focused);
        self.replace_terminal_after_reconnect(terminal, resize, cx);
        true
    }

    pub(super) fn can_reconnect_serial(&self) -> bool {
        self.serial_reconnect_config.is_some() && self.terminal_exited
    }

    pub(super) fn reconnect_serial(&mut self, cx: &mut Context<Self>) {
        if !self.can_reconnect_serial() {
            return;
        }
        let Some(config) = self.serial_reconnect_config.clone() else {
            return;
        };

        let resize = self
            .last_pty_resize
            .unwrap_or((DEFAULT_COLS, DEFAULT_ROWS, 0, 0));
        let runtime_options = self
            .terminal
            .lock()
            .serial_runtime_options()
            .unwrap_or_default();
        self.terminal.lock().shutdown();

        let mut terminal = match TerminalSession::serial_with_graphics_and_encoding(
            config.clone(),
            resize.0,
            resize.1,
            graphics_options_from_preferences(&self.preferences),
            self.preferences.terminal_encoding,
            self.preferences.scrollback_lines,
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                self.title = SharedString::from(format!(
                    "{}: {error}",
                    self.preferences.serial_control_labels.reconnect_failed
                ));
                cx.notify();
                return;
            }
        };
        let _ = terminal.set_serial_runtime_options(runtime_options);
        if resize.2 > 0 && resize.3 > 0 {
            let _ = terminal.resize_with_cell_size(resize.0, resize.1, resize.2, resize.3);
        }
        let _ = terminal.set_focused(self.focused);

        self.serial_reconnect_config = Some(config);
        self.serial_port_available = Some(true);
        self.replace_terminal_after_reconnect(terminal, resize, cx);
    }

    fn replace_terminal_after_reconnect(
        &mut self,
        terminal: TerminalSession,
        resize: (usize, usize, u16, u16),
        cx: &mut Context<Self>,
    ) {
        debug_assert_eq!(terminal.kind(), self.session_kind);
        let snapshot = terminal.snapshot();

        self.terminal = Arc::new(Mutex::new(terminal));
        self.snapshot = self.stamp_snapshot(snapshot);
        self.mark_terminal_content_changed(cx);
        self.terminal_exited = false;
        self.input_locked = false;
        self.title = SharedString::from("OxideTerm");
        self.selection = None;
        self.pending_paste = None;
        self.context_menu = None;
        self.context_action_requested = None;
        self.marked_text = None;
        self.privilege_prompt_inline_hint = None;
        self.privilege_prompt_submit_requested = false;
        self.search_query = None;
        self.search_cache = None;
        self.selected_search_match = None;
        self.hovered_link = None;
        self.hovered_command_mark_id = None;
        self.selecting = false;
        self.last_mouse_report_point = None;
        self.command_marks.clear();
        self.command_marks_render_cache_dirty = true;
        self.selected_command_mark_id = None;
        self.command_mark_id_aliases.clear();
        self.input_tracker.reset();
        self.privilege_prompt_tracker = PrivilegePromptTracker::default();
        self.privilege_prompt_expiry_generation =
            self.privilege_prompt_expiry_generation.wrapping_add(1);
        self.privilege_prompt_expiry_task = None;
        self.sync_terminal_output_events_enabled();
        cx.emit(TerminalPaneEvent::PrivilegePromptStateChanged);
        self.command_fact_ledger = CommandFactLedger::default();
        self.last_pty_resize = Some(resize);
        self.pending_pty_resize = None;
        self.last_drain_budget_exhausted = false;
        self.clear_smooth_scroll_remainder();
        self.reset_cursor_blink();
        self.wake_terminal_scheduler();
        cx.notify();
    }
}
