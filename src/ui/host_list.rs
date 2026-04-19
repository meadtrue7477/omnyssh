use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{AppAction, AppState, HostForm, HostListView, HostPopup, ViewState};
use crate::ssh::client::ConnectionStatus;
use crate::ui::popup;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Renders the host-list panel with search bar, host rows, and any active popup.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let hlv = &view.host_list;

    let total = hlv.filtered_indices.len();
    let all = state.hosts.len();
    let title = if hlv.search_mode || !hlv.search_query.is_empty() {
        format!(" Hosts ({}/{}) ", total, all)
    } else {
        format!(" Hosts ({}) ", all)
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(view.theme.text_success));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner area: 1-line search bar + rest for list.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    render_search_bar(
        frame,
        chunks[0],
        hlv.search_mode,
        &hlv.search_query,
        &view.theme,
    );

    if state.hosts.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No hosts. Press  a  to add one, or  r  to reload from ~/.ssh/config.",
                Style::default().fg(view.theme.text_muted),
            ))),
            chunks[1],
        );
    } else if hlv.filtered_indices.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No hosts match the search. Press  Esc  to clear.",
                Style::default().fg(view.theme.text_muted),
            ))),
            chunks[1],
        );
    } else {
        render_host_list(frame, chunks[1], state, view);
    }

    // Popups are drawn on top of everything.
    match &hlv.popup {
        Some(HostPopup::Add(form)) => popup::render_host_form(frame, form, "Add Host", &view.theme),
        Some(HostPopup::Edit { form, .. }) => {
            popup::render_host_form(frame, form, "Edit Host", &view.theme)
        }
        Some(HostPopup::DeleteConfirm(idx)) => {
            let name = state
                .hosts
                .get(*idx)
                .map(|h| h.name.as_str())
                .unwrap_or("?");
            popup::render_delete_confirm(frame, name, &view.theme);
        }
        Some(HostPopup::KeySetupConfirm(idx)) => {
            let host = state.hosts.get(*idx);
            popup::render_key_setup_confirm(frame, host, &view.theme);
        }
        Some(HostPopup::KeySetupProgress {
            host_name,
            current_step,
            ..
        }) => {
            popup::render_key_setup_progress(frame, host_name, current_step.as_ref(), &view.theme);
        }
        None => {}
    }
}

fn render_search_bar(
    frame: &mut Frame,
    area: Rect,
    active: bool,
    query: &str,
    theme: &crate::ui::theme::Theme,
) {
    let (prefix, prefix_style) = if active {
        (
            " [SEARCH] ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if !query.is_empty() {
        (" [filter] ", Style::default().fg(theme.accent))
    } else {
        ("  / search  ", Style::default().fg(theme.text_muted))
    };

    let cursor = if active { "|" } else { "" };
    let suffix = if active {
        "  (Enter: confirm  Esc: clear)"
    } else if !query.is_empty() {
        "  (/ to edit)"
    } else {
        ""
    };

    let line = Line::from(vec![
        Span::styled(prefix, prefix_style),
        Span::styled(
            format!("{}{}", query, cursor),
            Style::default().fg(theme.text_primary),
        ),
        Span::styled(suffix, Style::default().fg(theme.text_muted)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn render_host_list(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let hlv = &view.host_list;

    let total = hlv.filtered_indices.len();
    let selected = hlv.selected.min(total.saturating_sub(1));

    // Only build ListItem structs for rows that will actually be rendered.
    // ratatui would otherwise allocate items for all filtered hosts on every frame.
    let visible_count = area.height as usize;
    let visible_start = if total <= visible_count {
        0
    } else {
        let half = visible_count / 2;
        let max_start = total - visible_count;
        selected.saturating_sub(half).min(max_start)
    };

    let items: Vec<ListItem> = hlv
        .filtered_indices
        .iter()
        .enumerate()
        .skip(visible_start)
        .take(visible_count)
        .filter_map(|(_, &idx)| {
            let h = state.hosts.get(idx)?;

            let status_span = match state.connection_statuses.get(&h.name) {
                Some(ConnectionStatus::Connected) => {
                    Span::styled("● ", Style::default().fg(view.theme.text_success))
                }
                Some(ConnectionStatus::Connecting) => {
                    Span::styled("◐ ", Style::default().fg(view.theme.text_warning))
                }
                Some(ConnectionStatus::Failed(_)) => {
                    Span::styled("✗ ", Style::default().fg(view.theme.text_error))
                }
                _ => Span::raw("  "),
            };

            let name_span = Span::styled(
                format!("{:<20}", truncate(&h.name, 20)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );

            let host_span = Span::styled(
                format!("{:<18}", truncate(&h.hostname, 18)),
                Style::default().fg(view.theme.accent),
            );

            let user_port = if h.port == 22 {
                h.user.clone()
            } else {
                format!("{}:{}", h.user, h.port)
            };
            let up_span = Span::styled(
                format!("{:<14}", truncate(&user_port, 14)),
                Style::default().fg(view.theme.text_secondary),
            );

            let tags_str: String = h
                .tags
                .iter()
                .map(|t| format!("[{}]", t))
                .collect::<Vec<_>>()
                .join(" ");
            let tags_span = Span::styled(
                truncate(&tags_str, 24).to_string(),
                Style::default().fg(view.theme.text_muted),
            );

            Some(ListItem::new(Line::from(vec![
                status_span,
                name_span,
                Span::raw(" "),
                host_span,
                Span::raw(" "),
                up_span,
                Span::raw(" "),
                tags_span,
            ])))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    // Selection is relative to the visible window, not the full list.
    let mut list_state = ListState::default();
    if !hlv.filtered_indices.is_empty() {
        list_state.select(Some(selected.saturating_sub(visible_start)));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Truncates a string to at most `max` characters.
fn truncate(s: &str, max: usize) -> &str {
    if s.chars().count() <= max {
        return s;
    }
    let byte_idx = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    &s[..byte_idx]
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handles host-list-specific key events and returns an optional action for
/// the main loop to execute.
pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let hlv = &mut view.host_list;

    if hlv.popup.is_some() {
        return handle_popup_input(key, hlv);
    }

    if hlv.search_mode {
        return handle_search_input(key, hlv);
    }

    handle_normal_input(key, hlv)
}

fn handle_normal_input(key: KeyEvent, hlv: &mut HostListView) -> Option<AppAction> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            hlv.select_next();
            None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            hlv.select_prev();
            None
        }
        KeyCode::Enter => hlv.selected_host_idx().map(AppAction::ConnectAt),
        KeyCode::Char('/') => {
            hlv.search_mode = true;
            None
        }
        KeyCode::Char('a') => {
            hlv.popup = Some(HostPopup::Add(HostForm::empty()));
            None
        }
        KeyCode::Char('e') => {
            if hlv.selected_host_idx().is_some() {
                Some(AppAction::OpenEditPopup)
            } else {
                None
            }
        }
        KeyCode::Char('d') => {
            if let Some(idx) = hlv.selected_host_idx() {
                hlv.popup = Some(HostPopup::DeleteConfirm(idx));
            }
            None
        }
        KeyCode::Char('K') => {
            if hlv.selected_host_idx().is_some() {
                Some(AppAction::StartKeySetup)
            } else {
                None
            }
        }
        KeyCode::Char('r') => Some(AppAction::ReloadHosts),
        _ => None,
    }
}

fn handle_search_input(key: KeyEvent, hlv: &mut HostListView) -> Option<AppAction> {
    match key.code {
        KeyCode::Enter => {
            hlv.search_mode = false;
            None
        }
        KeyCode::Esc => {
            hlv.search_mode = false;
            hlv.search_query.clear();
            Some(AppAction::SearchQueryChanged)
        }
        KeyCode::Backspace => {
            hlv.search_query.pop();
            Some(AppAction::SearchQueryChanged)
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            hlv.search_query.push(c);
            Some(AppAction::SearchQueryChanged)
        }
        _ => None,
    }
}

fn handle_popup_input(key: KeyEvent, hlv: &mut HostListView) -> Option<AppAction> {
    // Esc always closes the popup regardless of type.
    if key.code == KeyCode::Esc {
        hlv.popup = None;
        return None;
    }

    match &mut hlv.popup {
        Some(HostPopup::Add(form)) | Some(HostPopup::Edit { form, .. }) => {
            handle_form_input(key, form)
        }
        Some(HostPopup::DeleteConfirm(_)) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(AppAction::ConfirmDelete),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                hlv.popup = None;
                None
            }
            _ => None,
        },
        Some(HostPopup::KeySetupConfirm(idx)) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                Some(AppAction::ConfirmKeySetup(*idx))
            }
            KeyCode::Char('n') | KeyCode::Char('N') => Some(AppAction::CancelKeySetup),
            _ => None,
        },
        Some(HostPopup::KeySetupProgress { .. }) => {
            // Progress popup is non-interactive, only Esc can close it
            None
        }
        None => None,
    }
}

fn handle_form_input(key: KeyEvent, form: &mut HostForm) -> Option<AppAction> {
    match key.code {
        KeyCode::Enter => Some(AppAction::ConfirmForm),
        // Esc is handled before we reach this function (in handle_popup_input).
        KeyCode::Tab => {
            form.focus_next();
            None
        }
        KeyCode::BackTab => {
            form.focus_prev();
            None
        }
        KeyCode::Backspace => {
            if let Some(field) = form.fields.get_mut(form.focused_field) {
                field.backspace();
            }
            None
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(field) = form.fields.get_mut(form.focused_field) {
                field.insert_char(c);
            }
            None
        }
        _ => None,
    }
}
