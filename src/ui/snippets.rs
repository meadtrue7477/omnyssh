//! Snippets screen — list of saved commands with CRUD and execution.
//!
//! Layout (schematic):
//!
//! ```text
//! ┌ header (1 line): "Snippets (N)"  [filter: X]  n:new  b:broadcast  /:search
//! ├─ list (rest of area) ────────────────────────────────────────────────────
//! │  [global]  Docker: restart all       docker compose down...  [docker]
//! │  [host:X]  Check replication lag     sudo -u postgres...
//! └───────────────────────────────────────────────────────────────────────────
//! (popups rendered as overlays via popup::*)
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::{AppAction, AppState, SnippetPopup, SnippetsView, ViewState};
use crate::config::snippets::SnippetScope;
use crate::ui::popup;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Renders the Snippets screen.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    if area.width < 40 || area.height < 6 {
        frame.render_widget(
            Paragraph::new("Terminal too small for snippets screen.")
                .style(Style::default().fg(view.theme.text_error)),
            area,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_header(frame, chunks[0], view);
    render_list(frame, chunks[1], state, view);

    // Overlay popups.
    let sv = &view.snippets_view;
    if let Some(popup_val) = &sv.popup {
        match popup_val {
            SnippetPopup::Add(form) => {
                popup::render_snippet_form(frame, form, "Add Snippet", &view.theme)
            }
            SnippetPopup::Edit { form, .. } => {
                popup::render_snippet_form(frame, form, "Edit Snippet", &view.theme)
            }
            SnippetPopup::DeleteConfirm(idx) => {
                let name = state
                    .snippets
                    .get(*idx)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?");
                popup::render_snippet_delete_confirm(frame, name, &view.theme);
            }
            SnippetPopup::ParamInput {
                snippet_idx,
                param_names,
                param_fields,
                focused_field,
                ..
            } => {
                let sname = state
                    .snippets
                    .get(*snippet_idx)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?");
                popup::render_param_input(
                    frame,
                    sname,
                    param_names,
                    param_fields,
                    *focused_field,
                    &view.theme,
                );
            }
            SnippetPopup::BroadcastPicker {
                selected_host_indices,
                cursor,
                ..
            } => {
                popup::render_broadcast_picker(
                    frame,
                    &state.hosts,
                    selected_host_indices,
                    *cursor,
                    &view.theme,
                );
            }
            // Results and QuickExecuteInput are rendered by ui/mod.rs overlay.
            _ => {}
        }
    }
}

fn render_header(frame: &mut Frame, area: Rect, view: &ViewState) {
    let sv = &view.snippets_view;
    let count = sv.filtered_indices.len();

    let mut spans: Vec<Span> = vec![Span::styled(
        format!(" Snippets ({}) ", count),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];

    if sv.search_mode {
        spans.push(Span::styled(
            format!("[search: {}] ", sv.search_query),
            Style::default().fg(view.theme.accent),
        ));
    } else if !sv.search_query.is_empty() {
        spans.push(Span::styled(
            format!("[filter: {}] ", sv.search_query),
            Style::default().fg(view.theme.text_warning),
        ));
    }

    spans.push(Span::styled(
        "  n:new  e:edit  d:del  Enter:run  b:broadcast  /:search",
        Style::default().fg(view.theme.text_muted),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_list(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    let sv = &view.snippets_view;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(view.theme.text_muted));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if sv.filtered_indices.is_empty() {
        let msg = if !sv.search_query.is_empty() {
            "  No snippets match."
        } else {
            "  No snippets. Press  n  to create your first snippet."
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ))),
            inner,
        );
        return;
    }

    let visible_height = inner.height as usize;
    // Clamp selected so it never exceeds the filtered list length.
    let selected = sv.selected.min(sv.filtered_indices.len().saturating_sub(1));
    // Scroll offset: keep `selected` visible.
    let offset = if selected >= visible_height {
        selected - visible_height + 1
    } else {
        0
    };

    let rows: Vec<Line> = sv
        .filtered_indices
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible_height)
        .map(|(row_idx, &snippet_idx)| {
            let s = &state.snippets[snippet_idx];
            let is_selected = row_idx == selected;

            // Scope badge.
            let (badge_text, badge_color) = match s.scope {
                SnippetScope::Global => ("global", Color::Cyan),
                SnippetScope::Host => ("host", Color::Yellow),
            };

            // Command preview — truncated.
            let cmd_preview: String = s.command.chars().take(40).collect();
            let cmd_display = if s.command.len() > 40 {
                format!("{}…", cmd_preview)
            } else {
                cmd_preview
            };

            // Tags.
            let tags_str = s
                .tags
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|t| format!("[{}]", t))
                .collect::<Vec<_>>()
                .join(" ");

            // Params indicator.
            let params_str = if s.params.as_deref().unwrap_or(&[]).is_empty() {
                String::new()
            } else {
                let names = s.params.as_deref().unwrap_or(&[]).join(", ");
                format!(" {{…{}…}}", names)
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(view.theme.text_primary)
            };

            // Build a multi-span Line for richer per-element styling.
            let scope_span = Span::styled(
                format!("  [{}", badge_text.trim()),
                Style::default()
                    .fg(badge_color)
                    .add_modifier(Modifier::BOLD),
            );
            let host_span = if s.scope == SnippetScope::Host {
                Span::styled(
                    format!(":{}]  ", s.host.as_deref().unwrap_or("?")),
                    Style::default().fg(view.theme.text_warning),
                )
            } else {
                Span::styled("]  ", Style::default().fg(badge_color))
            };

            let name_span = Span::styled(format!("{:<28}", s.name), name_style);
            let cmd_span = Span::styled(
                format!("  {}", cmd_display),
                Style::default().fg(view.theme.text_muted),
            );
            let params_span =
                Span::styled(params_str, Style::default().fg(view.theme.text_warning));
            let tags_span = if !tags_str.is_empty() {
                Span::styled(
                    format!("  {}", tags_str),
                    Style::default().fg(view.theme.accent),
                )
            } else {
                Span::raw("")
            };

            let mut line = Line::from(vec![
                scope_span,
                host_span,
                name_span,
                cmd_span,
                params_span,
                tags_span,
            ]);

            if is_selected {
                line = line.style(Style::default().bg(view.theme.selected_bg));
            }

            line
        })
        .collect();

    for (i, row) in rows.into_iter().enumerate() {
        if i >= visible_height {
            break;
        }
        let row_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(row), row_area);
    }
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handles key events for the Snippets screen and all its popups.
///
/// Returns the action to be processed by the main event loop, or `None`.
pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    let sv = &mut view.snippets_view;

    // Search mode.
    if sv.search_mode {
        return handle_search_input(key, sv);
    }

    // Popup mode.
    if sv.popup.is_some() {
        return handle_popup_input(key, view);
    }

    // Normal mode.
    handle_normal_input(key, sv)
}

fn handle_normal_input(key: KeyEvent, sv: &mut SnippetsView) -> Option<AppAction> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            sv.select_next();
            None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            sv.select_prev();
            None
        }
        KeyCode::Char('n') | KeyCode::Char('a') => Some(AppAction::OpenSnippetAdd),
        KeyCode::Char('e') => Some(AppAction::OpenSnippetEdit),
        KeyCode::Char('d') => Some(AppAction::OpenSnippetDeleteConfirm),
        KeyCode::Enter | KeyCode::Char('x') => {
            sv.selected_snippet_idx()
                .map(|idx| AppAction::ExecuteSnippet {
                    snippet_idx: idx,
                    host_names: vec![],
                })
        }
        KeyCode::Char('b') => Some(AppAction::OpenBroadcastPicker),
        KeyCode::Char('/') => {
            sv.search_mode = true;
            None
        }
        KeyCode::Esc => {
            if !sv.search_query.is_empty() {
                sv.search_query.clear();
                Some(AppAction::SnippetSearchChanged)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn handle_search_input(key: KeyEvent, sv: &mut SnippetsView) -> Option<AppAction> {
    match key.code {
        KeyCode::Esc => {
            sv.search_mode = false;
            sv.search_query.clear();
            Some(AppAction::SnippetSearchChanged)
        }
        KeyCode::Enter => {
            sv.search_mode = false;
            None
        }
        KeyCode::Backspace => {
            sv.search_query.pop();
            Some(AppAction::SnippetSearchChanged)
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            sv.search_query.push(c);
            Some(AppAction::SnippetSearchChanged)
        }
        _ => None,
    }
}

fn handle_popup_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    // Handle Esc for form popups before taking a mutable borrow on the popup.
    if key.code == KeyCode::Esc {
        let sv = &mut view.snippets_view;
        let close = matches!(
            sv.popup,
            Some(SnippetPopup::Add(_)) | Some(SnippetPopup::Edit { .. })
        );
        if close {
            sv.popup = None;
            return None;
        }
    }

    let sv = &mut view.snippets_view;

    match &mut sv.popup {
        Some(SnippetPopup::Add(form)) => handle_form_key(key, form, AppAction::ConfirmSnippetForm),
        Some(SnippetPopup::Edit { form, .. }) => {
            handle_form_key(key, form, AppAction::ConfirmSnippetForm)
        }
        Some(SnippetPopup::DeleteConfirm(_)) => match key.code {
            KeyCode::Char('y') => Some(AppAction::ConfirmSnippetDelete),
            KeyCode::Char('n') | KeyCode::Esc => {
                sv.popup = None;
                None
            }
            _ => None,
        },
        Some(SnippetPopup::ParamInput {
            param_fields,
            focused_field,
            ..
        }) => {
            let n = param_fields.len();
            match key.code {
                KeyCode::Enter => Some(AppAction::ConfirmParamInput),
                KeyCode::Esc => {
                    sv.popup = None;
                    None
                }
                KeyCode::Tab => {
                    *focused_field = (*focused_field + 1) % n.max(1);
                    None
                }
                KeyCode::BackTab => {
                    *focused_field = if *focused_field == 0 {
                        n.saturating_sub(1)
                    } else {
                        *focused_field - 1
                    };
                    None
                }
                KeyCode::Backspace => {
                    let f = *focused_field;
                    param_fields[f].backspace();
                    None
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let f = *focused_field;
                    param_fields[f].insert_char(c);
                    None
                }
                _ => None,
            }
        }
        Some(SnippetPopup::BroadcastPicker {
            selected_host_indices: _,
            cursor,
            snippet_idx,
        }) => {
            // We need host count from app state, but we only have view here.
            // Use a local copy for navigation — the actual host list count
            // is checked in process_action when ToggleBroadcastHost fires.
            let idx = *snippet_idx;
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    *cursor = cursor.saturating_add(1);
                    // Clamping happens in render (no host count available here).
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    None
                }
                KeyCode::Char(' ') => {
                    let c = *cursor;
                    Some(AppAction::ToggleBroadcastHost(c))
                }
                KeyCode::Enter => Some(AppAction::ConfirmBroadcast),
                KeyCode::Esc => {
                    sv.popup = None;
                    None
                }
                _ => {
                    let _ = idx;
                    None
                }
            }
        }
        Some(SnippetPopup::QuickExecuteInput {
            host_name,
            command_field,
        }) => match key.code {
            KeyCode::Enter => {
                let host = host_name.clone();
                let cmd = command_field.value.trim().to_string();
                sv.popup = None;
                if cmd.is_empty() {
                    None
                } else {
                    Some(AppAction::QuickExecute {
                        host_name: host,
                        command: cmd,
                    })
                }
            }
            KeyCode::Esc => {
                sv.popup = None;
                None
            }
            KeyCode::Backspace => {
                command_field.backspace();
                None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                command_field.insert_char(c);
                None
            }
            _ => None,
        },
        Some(SnippetPopup::Results { scroll, .. }) => match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                *scroll = scroll.saturating_add(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                *scroll = scroll.saturating_sub(1);
                None
            }
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                Some(AppAction::DismissSnippetResult)
            }
            _ => None,
        },
        None => None,
    }
}

/// Generic form key handler shared by Add and Edit snippet forms.
fn handle_form_key(
    key: KeyEvent,
    form: &mut crate::app::SnippetForm,
    confirm_action: AppAction,
) -> Option<AppAction> {
    match key.code {
        KeyCode::Enter => Some(confirm_action),
        // Esc is handled before entering this function (see handle_popup_input).
        KeyCode::Esc => None,
        KeyCode::Tab => {
            form.focus_next();
            None
        }
        KeyCode::BackTab => {
            form.focus_prev();
            None
        }
        KeyCode::Backspace => {
            form.fields[form.focused_field].backspace();
            None
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            form.fields[form.focused_field].insert_char(c);
            None
        }
        _ => None,
    }
}
