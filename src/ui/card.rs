//! Server card rendering primitives for the dashboard grid.
//!
//! A card displays a single host's name, connection status, and live
//! metrics (CPU / RAM / Disk) inside a bordered ratatui [`Block`].

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::event::{Alert, DetectedService, Metrics, ServiceKind};
use crate::ssh::client::ConnectionStatus;
use crate::ssh::metrics::threshold_color;
use crate::ui::theme::Theme;

// ---------------------------------------------------------------------------
// Card dimensions (kept in sync with dashboard.rs column calculation)
// ---------------------------------------------------------------------------

/// Minimum card width (characters), including borders.
/// Increased to 34 to properly display enriched format (A.4.1).
pub const CARD_MIN_WIDTH: u16 = 34;
/// Fixed card height (lines), including borders (increased for services display).
pub const CARD_HEIGHT: u16 = 10;

// ---------------------------------------------------------------------------
// Status indicators
// ---------------------------------------------------------------------------

fn status_dot(status: Option<&ConnectionStatus>) -> (&'static str, Color) {
    match status {
        Some(ConnectionStatus::Connected) => ("●", Color::Green),
        Some(ConnectionStatus::Connecting) => ("◐", Color::Yellow),
        Some(ConnectionStatus::Failed(_)) => ("✗", Color::Red),
        Some(ConnectionStatus::Unknown) | None => ("?", Color::DarkGray),
    }
}

// ---------------------------------------------------------------------------
// Public render function
// ---------------------------------------------------------------------------

/// Host display data passed to [`render_card`].
pub struct CardData<'a> {
    pub host_name: &'a str,
    pub hostname: &'a str,
    pub user: &'a str,
    pub port: u16,
    pub tags: &'a [String],
    pub metrics: Option<&'a Metrics>,
    pub status: Option<&'a ConnectionStatus>,
    /// Detected services.
    pub services: Option<&'a [DetectedService]>,
    /// Active alerts.
    pub alerts: Option<&'a [Alert]>,
}

/// Render a single server card into `rect`.
///
/// `is_selected` highlights the card border using the active theme accent
/// colour and uses thick double borders.
pub fn render_card(
    frame: &mut Frame,
    rect: Rect,
    data: &CardData<'_>,
    is_selected: bool,
    theme: &Theme,
) {
    let host_name = data.host_name;
    let hostname = data.hostname;
    let user = data.user;
    let port = data.port;
    let tags = data.tags;
    let metrics = data.metrics;
    let status = data.status;
    // ---- Border ----
    let (dot, dot_color) = status_dot(status);
    let title = format!(
        " {} ",
        truncate(host_name, rect.width.saturating_sub(6) as usize)
    );
    let title_right = format!(" {} ", dot);

    let border_color = if is_selected {
        theme.accent
    } else {
        theme.border
    };
    let title_color = if is_selected {
        theme.accent
    } else {
        theme.title
    };
    let border_type = if is_selected {
        BorderType::Double
    } else {
        BorderType::Rounded
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Left)
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .title_top(
            Line::from(Span::styled(title_right, Style::default().fg(dot_color)))
                .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // ---- Inner layout: 8 rows (Enriched format per A.4.1) ----
    // Row 0: hostname + user:port
    // Row 1: CPU + RAM (combined)
    // Row 2: DSK + Uptime (combined)
    // Row 3: horizontal separator
    // Row 4: services
    // Row 5: alerts
    // Row 6: alerts continued
    // Row 7: tags
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // hostname
            Constraint::Length(1), // cpu + ram
            Constraint::Length(1), // disk + uptime
            Constraint::Length(1), // separator
            Constraint::Length(1), // services
            Constraint::Length(1), // alerts line 1
            Constraint::Length(1), // alerts line 2
            Constraint::Length(1), // tags
            Constraint::Min(0),    // remainder (safety)
        ])
        .split(inner);

    // Row 0: hostname + user:port
    let user_port = format!("{}:{}", user, port);
    let hostname_trunc = truncate(
        hostname,
        inner.width.saturating_sub(user_port.len() as u16 + 1) as usize,
    );
    let addr_line = Line::from(vec![
        Span::styled(hostname_trunc, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::styled(user_port, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(addr_line), rows[0]);

    // Check if offline
    let is_offline = matches!(
        status,
        Some(ConnectionStatus::Failed(_)) | Some(ConnectionStatus::Unknown) | None
    ) && metrics.is_none();

    if is_offline {
        // Rows 1-2: offline message
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─── offline ───",
                Style::default().fg(Color::Red),
            ))),
            rows[1],
        );
    } else {
        // Row 1: CPU + RAM combined (A.4.1 spec: "CPU: ████░░ 73%  RAM: 2.1/4GB")
        let cpu = metrics.and_then(|m| m.cpu_percent);
        let ram = metrics.and_then(|m| m.ram_percent);
        frame.render_widget(
            Paragraph::new(render_cpu_ram_line(cpu, ram, inner.width)),
            rows[1],
        );

        // Row 2: DSK + Uptime combined (A.4.1 spec: "DSK: ████░░ 61%  Up: 43 days")
        let disk = metrics.and_then(|m| m.disk_percent);
        let uptime_str = metrics.and_then(|m| m.uptime.as_deref()).unwrap_or("");
        frame.render_widget(
            Paragraph::new(render_disk_uptime_line(disk, uptime_str, inner.width)),
            rows[2],
        );
    }

    // Row 3: horizontal separator (A.4.1 spec: "│───────────────────────────────│")
    let separator = "─".repeat(inner.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            separator,
            Style::default().fg(Color::DarkGray),
        ))),
        rows[3],
    );

    // Rows 4-5: services
    // Use TWO lines for services to avoid "+N" overflow
    let mut service_row_offset = 0;
    if let Some(services) = data.services {
        if !services.is_empty() {
            let service_lines = render_services_lines(services, inner.width, 2);
            for (i, line) in service_lines.iter().enumerate() {
                if i < 2 {
                    frame.render_widget(Paragraph::new(line.clone()), rows[4 + i]);
                    service_row_offset = i + 1;
                }
            }
        }
    }

    // Rows 6-7 (or 5-6 if no services): alerts
    // Show up to 2 alert lines for important issues, but shift down if services used both rows
    if let Some(alerts) = data.alerts {
        if !alerts.is_empty() {
            let alert_start_row = 4 + service_row_offset;
            let alert_lines = render_alert_lines(alerts, inner.width, 2);
            for (i, line) in alert_lines.iter().enumerate() {
                let row_idx = alert_start_row + i;
                if row_idx < 7 {
                    // Don't overlap with tags row
                    frame.render_widget(Paragraph::new(line.clone()), rows[row_idx]);
                }
            }
        }
    }

    // Row 7: tags
    if !tags.is_empty() {
        let tag_spans: Vec<Span> = tags
            .iter()
            .flat_map(|t| {
                [
                    Span::styled("[", Style::default().fg(Color::DarkGray)),
                    Span::styled(t.as_str(), Style::default().fg(Color::Gray)),
                    Span::styled("] ", Style::default().fg(Color::DarkGray)),
                ]
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(tag_spans)), rows[7]);
    }
}

// ---------------------------------------------------------------------------
// Combined metric line rendering (A.4.1 Enriched format)
// ---------------------------------------------------------------------------

/// Render CPU + RAM on one line: "CPU: ████░░ 73%  RAM: 2.1/4GB"
fn render_cpu_ram_line(cpu: Option<f64>, ram: Option<f64>, _width: u16) -> Line<'static> {
    let mut spans = Vec::new();

    // CPU part with compact bar
    spans.push(Span::styled("CPU: ", Style::default().fg(Color::Gray)));
    if let Some(pct) = cpu {
        let bar_width = 6; // Fixed short bar
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let filled = filled.min(bar_width);
        let mut bar = String::with_capacity(bar_width * 3);
        for _ in 0..filled {
            bar.push('█');
        }
        for _ in filled..bar_width {
            bar.push('░');
        }
        let color = threshold_color(pct);
        spans.push(Span::styled(bar, Style::default().fg(color)));
        spans.push(Span::styled(
            format!(" {:>3.0}%", pct),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            "░░░░░░ --",
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans.push(Span::raw("  "));

    // RAM part with compact bar
    spans.push(Span::styled("RAM: ", Style::default().fg(Color::Gray)));
    if let Some(pct) = ram {
        let bar_width = 6;
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let filled = filled.min(bar_width);
        let mut bar = String::with_capacity(bar_width * 3);
        for _ in 0..filled {
            bar.push('█');
        }
        for _ in filled..bar_width {
            bar.push('░');
        }
        let color = threshold_color(pct);
        spans.push(Span::styled(bar, Style::default().fg(color)));
        spans.push(Span::styled(
            format!(" {:>3.0}%", pct),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            "░░░░░░ --",
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

/// Render Disk + Uptime on one line: "DSK: ████░░ 61%  Up: 43 days"
fn render_disk_uptime_line(disk: Option<f64>, uptime: &str, _width: u16) -> Line<'static> {
    let mut spans = Vec::new();

    // Disk part with compact bar
    spans.push(Span::styled("DSK: ", Style::default().fg(Color::Gray)));
    if let Some(pct) = disk {
        let bar_width = 6;
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let filled = filled.min(bar_width);
        let mut bar = String::with_capacity(bar_width * 3);
        for _ in 0..filled {
            bar.push('█');
        }
        for _ in filled..bar_width {
            bar.push('░');
        }
        let color = threshold_color(pct);
        spans.push(Span::styled(bar, Style::default().fg(color)));
        spans.push(Span::styled(
            format!(" {:>3.0}%", pct),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            "░░░░░░ --",
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans.push(Span::raw("  "));

    // Uptime part - no truncation, always show full uptime
    if !uptime.is_empty() {
        let uptime_display = format!("Up: {}", uptime);
        spans.push(Span::styled(
            uptime_display,
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Service and Alert rendering
// ---------------------------------------------------------------------------

/// Render services, ONE service per line (user requirement).
/// Returns up to `max_lines` lines with services displayed.
/// Example line: "🐳 Docker: 4 running"
fn render_services_lines(
    services: &[DetectedService],
    width: u16,
    max_lines: usize,
) -> Vec<Line<'static>> {
    use crate::event::ServiceStatus;

    let mut lines = Vec::new();

    for service in services.iter().take(max_lines) {
        let (icon, base_color) = service_icon(&service.kind);

        // Color by status: red for critical, yellow for degraded, green for healthy
        let color = match &service.status {
            ServiceStatus::Critical(_) => Color::Red,
            ServiceStatus::Degraded(_) => Color::Yellow,
            ServiceStatus::Healthy => base_color,
            ServiceStatus::Unknown => Color::DarkGray,
        };

        // Format: "🐳 Docker: 4 running"
        let service_name = service_name_short(&service.kind);
        let info = service_info(service);

        // Build line with icon, name, and info (truncate if too long)
        let text = if !info.is_empty() {
            format!("{} {}: {}", icon, service_name, info)
        } else {
            // If no info (e.g., systemd with no metrics), skip this service
            continue;
        };

        // Truncate to fit width
        let truncated = truncate(&text, (width as usize).saturating_sub(1));

        lines.push(Line::from(Span::styled(
            truncated,
            Style::default().fg(color),
        )));
    }

    lines
}

/// Render alert lines (A.4.1 format).
/// Example: "⚠ nginx-proxy restarting (x5)"
fn render_alert_lines(alerts: &[Alert], width: u16, max_lines: usize) -> Vec<Line<'static>> {
    use crate::event::AlertSeverity;

    let mut lines = Vec::new();

    // Prioritize: critical first, then warnings, then info
    let mut sorted_alerts = alerts.to_vec();
    sorted_alerts.sort_by(|a, b| b.severity.cmp(&a.severity));

    for alert in sorted_alerts.iter().take(max_lines) {
        let (icon, color) = match alert.severity {
            AlertSeverity::Critical => ("⚠", Color::Red),
            AlertSeverity::Warning => ("⚠", Color::Yellow),
            AlertSeverity::Info => ("ℹ", Color::Cyan),
        };

        let msg = truncate(&alert.message, width.saturating_sub(3) as usize);
        lines.push(Line::from(vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(msg, Style::default().fg(color)),
        ]));
    }

    lines
}

/// Get icon and color for a service kind.
fn service_icon(kind: &ServiceKind) -> (&'static str, Color) {
    match kind {
        ServiceKind::Docker => ("🐳", Color::Cyan),
        ServiceKind::Nginx => ("🌐", Color::Green),
        ServiceKind::PostgreSQL => ("🐘", Color::Blue),
        ServiceKind::Redis => ("📦", Color::Red),
        ServiceKind::NodeJS => ("🟢", Color::Green),
    }
}

/// Get short service name for display (A.4.1 format).
fn service_name_short(kind: &ServiceKind) -> &str {
    match kind {
        ServiceKind::Docker => "Docker",
        ServiceKind::Nginx => "Nginx",
        ServiceKind::PostgreSQL => "PG",
        ServiceKind::Redis => "Redis",
        ServiceKind::NodeJS => "Node",
    }
}

/// Extract detailed info for a service per A.4.1 spec.
/// Examples: "8 containers", "repl lag 2.3s", "0 errors/5min"
fn service_info(service: &DetectedService) -> String {
    use crate::event::MetricValue;

    match service.kind {
        ServiceKind::Docker => {
            // Show detailed container breakdown: "4 running, 2 stopped, 1 restarting"
            if service.metrics.is_empty() {
                return String::new(); // Deep Probe hasn't run yet
            }

            let mut running = 0i64;
            let mut stopped = 0i64;
            let mut restarting = 0i64;

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
                    "containers_restarting" => {
                        if let MetricValue::Integer(n) = metric.value {
                            restarting = n;
                        }
                    }
                    _ => {}
                }
            }

            let total = running + stopped + restarting;
            if total == 0 {
                return String::from("no containers");
            }

            // Build status string with only non-zero counts
            let mut parts = Vec::new();
            if running > 0 {
                parts.push(format!("{} running", running));
            }
            if stopped > 0 {
                parts.push(format!("{} stopped", stopped));
            }
            if restarting > 0 {
                parts.push(format!("{} restarting", restarting));
            }

            parts.join(", ")
        }
        ServiceKind::PostgreSQL => {
            // Show replication lag if present (A.4.1: "repl lag 2.3s")
            for metric in &service.metrics {
                if metric.name == "replication_lag_seconds" {
                    if let MetricValue::Integer(lag) = metric.value {
                        if lag > 0 {
                            return format!("repl lag {}s", lag);
                        }
                    } else if let MetricValue::Float(lag) = metric.value {
                        if lag > 0.0 {
                            return format!("repl lag {:.1}s", lag);
                        }
                    }
                }
            }
            String::from("ok")
        }
        ServiceKind::Nginx => {
            // Show error count (A.4.1 format)
            for metric in &service.metrics {
                if metric.name == "recent_502_504_errors" {
                    if let MetricValue::Integer(errors) = metric.value {
                        if errors > 0 {
                            return format!("{} errors/5min", errors);
                        }
                    }
                }
            }
            String::from("ok")
        }
        ServiceKind::Redis => {
            // Show memory usage
            let mut mem_used = 0i64;
            for metric in &service.metrics {
                if metric.name == "memory_used_mb" {
                    if let MetricValue::Integer(mb) = metric.value {
                        mem_used = mb;
                    }
                }
            }
            if mem_used > 0 {
                format!("{}MB used", mem_used)
            } else {
                String::from("ok")
            }
        }
        ServiceKind::NodeJS => {
            // Show node processes count
            let mut node_processes = 0i64;
            for metric in &service.metrics {
                if metric.name == "node_processes" {
                    if let MetricValue::Integer(count) = metric.value {
                        node_processes = count;
                    }
                }
            }
            if node_processes > 0 {
                format!("{} process(es)", node_processes)
            } else {
                String::from("no processes")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // Walk char boundaries without collecting into a Vec<char>.
    let mut iter = s.char_indices();
    match iter.nth(max_chars.saturating_sub(1)) {
        // Fewer than max_chars characters — return as-is.
        None => s.to_string(),
        Some((byte_pos, _)) => {
            if iter.next().is_none() {
                // Exactly max_chars characters — return as-is.
                s.to_string()
            } else {
                // More than max_chars characters — truncate and append ellipsis.
                format!("{}…", &s[..byte_pos])
            }
        }
    }
}
