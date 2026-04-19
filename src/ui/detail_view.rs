//! Detail View screen — Smart Server Context.
//!
//! Shows comprehensive server information before SSH connection,
//! including metrics, services, alerts, and suggested actions.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{AppAction, AppState, SnippetPopup, ViewState};
use crate::event::{Alert, AlertSeverity, DetectedService, Metrics, ServiceKind, ServiceStatus};
use crate::ssh::client::ConnectionStatus;
use crate::ssh::metrics::threshold_color;
use crate::ui::theme::Theme;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Renders the Detail View screen for the selected host.
///
/// Layout per A.4.2 spec:
/// ```text
/// ╔══ web-prod-1 ═══════════════════════════════════════════════╗
/// ║ Host: 192.168.1.10:22   User: deploy   Up: 43 days         ║
/// ║ OS: Ubuntu 22.04 LTS    Kernel: 5.15.0-91                  ║
/// ╠═════════════════════════════════════════════════════════════╣
/// ║ METRICS                          │ ALERTS                   ║
/// ║ CPU: ████████░░░░ 73%            │ ⚠ nginx-proxy restart x5 ║
/// ║ RAM: ██████░░░░░░ 2.1/4 GB      │ ⚠ Disk /var > 85%        ║
/// ║ DSK: ████████░░░░ 61%           │                          ║
/// ║ Load: 2.4 1.8 1.2               │                          ║
/// ╠═════════════════════════════════════════════════════════════╣
/// ║ SERVICES                                                    ║
/// ║ 🐳 Docker         8 running, 1 stopped    [containers: F4] ║
/// ║ 🌐 Nginx          active, 0 errors/5min   [logs: F5]       ║
/// ║ 🐘 PostgreSQL 16  repl lag: 2.3s          [queries: F6]    ║
/// ╠═════════════════════════════════════════════════════════════╣
/// ║ SUGGESTED ACTIONS                                           ║
/// ║ [ ] Docker: restart nginx-proxy                             ║
/// ║ [ ] Check Nginx upstream config                             ║
/// ╚═════════════════════════════════════════════════════════════╝
/// ```
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, view: &ViewState) {
    // Minimum terminal size guard
    if area.width < 60 || area.height < 20 {
        frame.render_widget(
            Paragraph::new("Terminal too small for detail view. (min 60x20)")
                .style(Style::default().fg(view.theme.text_error)),
            area,
        );
        return;
    }

    // Get the selected host
    let host_idx = match view.host_list.selected_host_idx() {
        Some(idx) => idx,
        None => {
            frame.render_widget(
                Paragraph::new("No host selected.")
                    .style(Style::default().fg(view.theme.text_error)),
                area,
            );
            return;
        }
    };

    let host = &state.hosts[host_idx];
    let metrics = state.metrics.get(&host.name);
    let status = state.connection_statuses.get(&host.name);
    let services = state.services.get(&host.name);
    let alerts = state.alerts.get(&host.name);

    // Main border with title
    let title = format!(" {} ", host.name);
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(view.theme.accent));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Layout: hints (1 line) + header (2 lines) + separator + metrics/alerts (6 lines) + separator + services (flex) + separator + actions (flex)
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Key hints header
            Constraint::Length(2), // Header (host info + OS info)
            Constraint::Length(1), // Separator
            Constraint::Length(6), // Metrics + Alerts (2 columns)
            Constraint::Length(1), // Separator
            Constraint::Min(5),    // Services section
            Constraint::Length(1), // Separator
            Constraint::Min(3),    // Suggested actions
        ])
        .split(inner);

    // Key hints header
    render_hints(frame, sections[0], &view.theme);

    // Header section
    render_header(frame, sections[1], host, metrics, status, &view.theme);

    // Separator
    render_separator(frame, sections[2], inner.width, &view.theme);

    // Metrics + Alerts (2 columns)
    render_metrics_alerts(
        frame,
        sections[3],
        metrics,
        alerts.map(|v| v.as_slice()),
        &view.theme,
    );

    // Separator
    render_separator(frame, sections[4], inner.width, &view.theme);

    // Services section
    if let Some(svcs) = services {
        render_services(frame, sections[5], svcs, &view.theme);
    } else {
        frame.render_widget(
            Paragraph::new("  SERVICES\n\n  No discovery data available. Press 'r' to refresh.")
                .style(Style::default().fg(view.theme.text_muted)),
            sections[5],
        );
    }

    // Separator
    render_separator(frame, sections[6], inner.width, &view.theme);

    // Suggested actions
    if let Some(alts) = alerts {
        render_suggested_actions(frame, sections[7], alts, &view.theme);
    }
}

// ---------------------------------------------------------------------------
// Header section
// ---------------------------------------------------------------------------

fn render_header(
    frame: &mut Frame,
    area: Rect,
    host: &crate::ssh::client::Host,
    metrics: Option<&Metrics>,
    status: Option<&ConnectionStatus>,
    theme: &Theme,
) {
    let status_text = match status {
        Some(ConnectionStatus::Connected) => ("Connected", Color::Green),
        Some(ConnectionStatus::Connecting) => ("Connecting", Color::Yellow),
        Some(ConnectionStatus::Failed(e)) => {
            let msg = format!("Failed: {}", e);
            (msg.leak() as &str, Color::Red)
        }
        _ => ("Unknown", Color::DarkGray),
    };

    let uptime = metrics
        .and_then(|m| m.uptime.as_deref())
        .unwrap_or("unknown");

    // Line 1: Host info
    let line1 = Line::from(vec![
        Span::styled(" Host: ", Style::default().fg(theme.text_secondary)),
        Span::styled(
            format!("{}:{}", host.hostname, host.port),
            Style::default().fg(theme.accent),
        ),
        Span::raw("   "),
        Span::styled("User: ", Style::default().fg(theme.text_secondary)),
        Span::styled(&host.user, Style::default().fg(theme.text_warning)),
        Span::raw("   "),
        Span::styled("Up: ", Style::default().fg(theme.text_secondary)),
        Span::styled(uptime, Style::default().fg(theme.text_success)),
        Span::raw("   "),
        Span::styled("Status: ", Style::default().fg(theme.text_secondary)),
        Span::styled(status_text.0, Style::default().fg(status_text.1)),
    ]);

    // Line 2: OS info from discovery
    let os_display = metrics
        .and_then(|m| m.os_info.as_deref())
        .unwrap_or("(discovery pending)");

    let line2 = Line::from(vec![
        Span::styled(" OS: ", Style::default().fg(theme.text_secondary)),
        Span::styled(
            os_display,
            if os_display == "(discovery pending)" {
                Style::default().fg(theme.text_muted)
            } else {
                Style::default().fg(theme.accent)
            },
        ),
    ]);

    let header_text = vec![line1, line2];
    frame.render_widget(Paragraph::new(header_text), area);
}

// ---------------------------------------------------------------------------
// Metrics + Alerts (2-column layout)
// ---------------------------------------------------------------------------

fn render_metrics_alerts(
    frame: &mut Frame,
    area: Rect,
    metrics: Option<&Metrics>,
    alerts: Option<&[Alert]>,
    theme: &Theme,
) {
    // Split into 2 columns
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: Metrics
    render_metrics_column(frame, columns[0], metrics, theme);

    // Right: Alerts
    render_alerts_column(frame, columns[1], alerts, theme);
}

fn render_metrics_column(frame: &mut Frame, area: Rect, metrics: Option<&Metrics>, theme: &Theme) {
    let mut lines = vec![Line::from(Span::styled(
        " METRICS",
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))];

    if let Some(m) = metrics {
        // CPU line with bar
        if let Some(cpu) = m.cpu_percent {
            let bar_width = 12;
            let filled = ((cpu / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let mut bar = String::with_capacity(bar_width * 3);
            for _ in 0..filled {
                bar.push('█');
            }
            for _ in filled..bar_width {
                bar.push('░');
            }
            let color = threshold_color(cpu);
            lines.push(Line::from(vec![
                Span::styled(" CPU: ", Style::default().fg(theme.text_secondary)),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(
                    format!(" {:>5.1}%", cpu),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // RAM line with bar
        if let Some(ram) = m.ram_percent {
            let bar_width = 12;
            let filled = ((ram / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let mut bar = String::with_capacity(bar_width * 3);
            for _ in 0..filled {
                bar.push('█');
            }
            for _ in filled..bar_width {
                bar.push('░');
            }
            let color = threshold_color(ram);
            lines.push(Line::from(vec![
                Span::styled(" RAM: ", Style::default().fg(theme.text_secondary)),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(
                    format!(" {:>5.1}%", ram),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Disk line with bar
        if let Some(disk) = m.disk_percent {
            let bar_width = 12;
            let filled = ((disk / 100.0) * bar_width as f64).round() as usize;
            let filled = filled.min(bar_width);
            let mut bar = String::with_capacity(bar_width * 3);
            for _ in 0..filled {
                bar.push('█');
            }
            for _ in filled..bar_width {
                bar.push('░');
            }
            let color = threshold_color(disk);
            lines.push(Line::from(vec![
                Span::styled(" DSK: ", Style::default().fg(theme.text_secondary)),
                Span::styled(bar, Style::default().fg(color)),
                Span::styled(
                    format!(" {:>5.1}%", disk),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        // Load average
        if let Some(load) = &m.load_avg {
            lines.push(Line::from(vec![
                Span::styled(" Load: ", Style::default().fg(theme.text_secondary)),
                Span::styled(load.as_str(), Style::default().fg(theme.accent)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            " (no metrics available)",
            Style::default().fg(theme.text_muted),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_alerts_column(frame: &mut Frame, area: Rect, alerts: Option<&[Alert]>, theme: &Theme) {
    let mut lines = vec![Line::from(Span::styled(
        "ALERTS",
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))];

    if let Some(alts) = alerts {
        if alts.is_empty() {
            lines.push(Line::from(Span::styled(
                " No alerts",
                Style::default().fg(theme.text_success),
            )));
        } else {
            // Show up to 5 alerts, prioritize critical first
            let mut sorted = alts.to_vec();
            sorted.sort_by(|a, b| b.severity.cmp(&a.severity));

            for alert in sorted.iter().take(5) {
                let (icon, color) = match alert.severity {
                    AlertSeverity::Critical => ("⚠", Color::Red),
                    AlertSeverity::Warning => ("⚠", Color::Yellow),
                    AlertSeverity::Info => ("ℹ", Color::Cyan),
                };

                lines.push(Line::from(vec![
                    Span::styled(format!(" {}", icon), Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(alert.message.clone(), Style::default().fg(color)),
                ]));
            }

            if alts.len() > 5 {
                lines.push(Line::from(Span::styled(
                    format!(" ... and {} more", alts.len() - 5),
                    Style::default().fg(theme.text_muted),
                )));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            " (no discovery data)",
            Style::default().fg(theme.text_muted),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

// ---------------------------------------------------------------------------
// Services section
// ---------------------------------------------------------------------------

fn render_services(frame: &mut Frame, area: Rect, services: &[DetectedService], theme: &Theme) {
    let mut lines = vec![Line::from(Span::styled(
        " SERVICES",
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))];

    if services.is_empty() {
        lines.push(Line::from(Span::styled(
            " No services detected",
            Style::default().fg(theme.text_muted),
        )));
    } else {
        for service in services.iter() {
            let (icon, base_color) = service_icon(&service.kind);
            let color = match &service.status {
                ServiceStatus::Critical(_) => Color::Red,
                ServiceStatus::Degraded(_) => Color::Yellow,
                ServiceStatus::Healthy => base_color,
                ServiceStatus::Unknown => Color::DarkGray,
            };

            let service_name = service_name_display(&service.kind);
            let status_info = service_status_display(service);
            // Show the correct hotkey based on service type, not position in list
            let f_key_hint = service_hotkey(&service.kind);

            let line = Line::from(vec![
                Span::raw(" "),
                Span::styled(icon, Style::default().fg(color)),
                Span::raw(" "),
                Span::styled(
                    format!("{:<15}", service_name),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(status_info, Style::default().fg(theme.text_secondary)),
                Span::raw("  "),
                Span::styled(f_key_hint, Style::default().fg(theme.text_muted)),
            ]);

            lines.push(line);
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// Get icon for service kind.
fn service_icon(kind: &ServiceKind) -> (&'static str, Color) {
    match kind {
        ServiceKind::Docker => ("🐳", Color::Cyan),
        ServiceKind::Nginx => ("🌐", Color::Green),
        ServiceKind::PostgreSQL => ("🐘", Color::Blue),
        ServiceKind::Redis => ("📦", Color::Red),
        ServiceKind::NodeJS => ("🟢", Color::Green),
    }
}

/// Get display name for service.
fn service_name_display(kind: &ServiceKind) -> String {
    match kind {
        ServiceKind::Docker => "Docker".to_string(),
        ServiceKind::Nginx => "Nginx".to_string(),
        ServiceKind::PostgreSQL => "PostgreSQL".to_string(),
        ServiceKind::Redis => "Redis".to_string(),
        ServiceKind::NodeJS => "Node.js".to_string(),
    }
}

/// Get the hotkey number for a service based on its type.
/// This ensures the correct hotkey is displayed regardless of service order.
fn service_hotkey(kind: &ServiceKind) -> String {
    let key_num = match kind {
        ServiceKind::Docker => 4,
        ServiceKind::Nginx => 5,
        ServiceKind::PostgreSQL => 6,
        ServiceKind::Redis => 7,
        ServiceKind::NodeJS => 8,
    };
    format!("[{}]", key_num)
}

/// Get status display for service (e.g., "8 running, 1 stopped", "repl lag: 2.3s").
fn service_status_display(service: &DetectedService) -> String {
    use crate::event::MetricValue;

    match service.kind {
        ServiceKind::Docker => {
            let mut running = 0i64;
            let mut stopped = 0i64;
            for metric in &service.metrics {
                match metric.name.as_str() {
                    "containers_running" => {
                        if let MetricValue::Integer(n) = metric.value {
                            running = n;
                        }
                    }
                    "containers_stopped" => {
                        if let MetricValue::Integer(n) = metric.value {
                            stopped = n;
                        }
                    }
                    _ => {}
                }
            }
            if stopped > 0 {
                format!("{} running, {} stopped", running, stopped)
            } else {
                format!("{} containers running", running)
            }
        }
        ServiceKind::PostgreSQL => {
            for metric in &service.metrics {
                if metric.name == "replication_lag_seconds" {
                    if let MetricValue::Integer(lag) = metric.value {
                        if lag > 0 {
                            return format!("repl lag: {}s", lag);
                        }
                    } else if let MetricValue::Float(lag) = metric.value {
                        if lag > 0.0 {
                            return format!("repl lag: {:.1}s", lag);
                        }
                    }
                }
            }
            "active, no replication lag".to_string()
        }
        ServiceKind::Nginx => {
            for metric in &service.metrics {
                if metric.name == "recent_502_504_errors" {
                    if let MetricValue::Integer(errors) = metric.value {
                        return format!("active, {} errors/5min", errors);
                    }
                }
            }
            "active, 0 errors/5min".to_string()
        }
        ServiceKind::Redis => {
            let mut mem_used = 0i64;
            for metric in &service.metrics {
                if metric.name.as_str() == "memory_used_mb" {
                    if let MetricValue::Integer(mem) = metric.value {
                        mem_used = mem;
                    }
                }
            }
            if mem_used > 0 {
                format!("active, {}MB mem", mem_used)
            } else {
                "active".to_string()
            }
        }
        ServiceKind::NodeJS => {
            let mut node_processes = 0i64;
            for metric in &service.metrics {
                if metric.name == "node_processes" {
                    if let MetricValue::Integer(count) = metric.value {
                        node_processes = count;
                    }
                }
            }
            if node_processes > 0 {
                format!("{} process(es) running", node_processes)
            } else {
                "no processes".to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Suggested Actions section
// ---------------------------------------------------------------------------

fn render_suggested_actions(frame: &mut Frame, area: Rect, alerts: &[Alert], theme: &Theme) {
    let mut lines = vec![Line::from(Span::styled(
        " SUGGESTED ACTIONS",
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))];

    // Filter alerts that have suggested actions
    let mut actions_with_suggestions = Vec::new();
    for alert in alerts.iter() {
        if alert.suggested_action.is_some() {
            actions_with_suggestions.push(alert);
        }
    }

    if actions_with_suggestions.is_empty() {
        lines.push(Line::from(Span::styled(
            " No suggested actions",
            Style::default().fg(theme.text_muted),
        )));
    } else {
        for action in actions_with_suggestions.iter().take(5) {
            if let Some(suggestion) = &action.suggested_action {
                lines.push(Line::from(vec![
                    Span::raw(" [ ] "),
                    Span::styled(suggestion.clone(), Style::default().fg(theme.accent)),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

// ---------------------------------------------------------------------------
// Separator
// ---------------------------------------------------------------------------

fn render_separator(frame: &mut Frame, area: Rect, width: u16, theme: &Theme) {
    let separator = "─".repeat(width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            separator,
            Style::default().fg(theme.text_muted),
        ))),
        area,
    );
}

// ---------------------------------------------------------------------------
// Key hints header
// ---------------------------------------------------------------------------

fn render_hints(frame: &mut Frame, area: Rect, theme: &Theme) {
    let hints = Line::from(vec![
        Span::styled(
            " Enter",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Connect", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "r",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Refresh", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":← Dashboard", Style::default().fg(theme.text_muted)),
        Span::raw("  "),
        Span::styled(
            "4-9",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(":Quick view", Style::default().fg(theme.text_muted)),
    ]);

    frame.render_widget(Paragraph::new(hints), area);
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handles key input for the Detail View screen.
pub fn handle_input(key: KeyEvent, view: &mut ViewState) -> Option<AppAction> {
    // If Results popup is open, handle it first
    if let Some(SnippetPopup::Results { scroll, .. }) = &mut view.snippets_view.popup {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                *scroll = scroll.saturating_add(1);
                return None;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                *scroll = scroll.saturating_sub(1);
                return None;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                return Some(AppAction::DismissSnippetResult);
            }
            _ => return None,
        }
    }

    match key.code {
        // Connect to host (same as Dashboard Enter)
        KeyCode::Enter => Some(AppAction::ConnectFromDetailView),

        // Refresh metrics and discovery
        KeyCode::Char('r') => Some(AppAction::RefreshMetrics),

        // Go back to Dashboard
        KeyCode::Esc => Some(AppAction::CloseDetailView),

        // Number keys for quick-view: 4=Docker, 5=Nginx, 6=PostgreSQL, 7=Redis, 8=Node.js
        KeyCode::Char('4') => Some(AppAction::ShowQuickView(ServiceKind::Docker)),
        KeyCode::Char('5') => Some(AppAction::ShowQuickView(ServiceKind::Nginx)),
        KeyCode::Char('6') => Some(AppAction::ShowQuickView(ServiceKind::PostgreSQL)),
        KeyCode::Char('7') => Some(AppAction::ShowQuickView(ServiceKind::Redis)),
        KeyCode::Char('8') => Some(AppAction::ShowQuickView(ServiceKind::NodeJS)),
        KeyCode::Char('9') => None, // Reserved for future use

        _ => None,
    }
}
