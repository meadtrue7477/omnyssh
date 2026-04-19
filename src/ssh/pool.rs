//! Background metrics polling pool.
//!
//! Each host gets its own persistent tokio task that manages its SSH
//! connection and collects metrics at a configurable interval.
//!
//! Architecture:
//! - [`PollManager`] — created by the main app, owns abort handles.
//! - One `HostPoller` task per host — loops indefinitely until aborted.
//! - Implements exponential backoff on connection failures.
//! - One SSH connection per host, reused across polls.
//! - All data sent to the main event loop via `mpsc::Sender<AppEvent>`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::event::{AppEvent, Metrics};
use crate::ssh::client::{ConnectionStatus, Host};
use crate::ssh::metrics::{
    parse_cpu_proc_stat, parse_cpu_top, parse_cpu_top_macos, parse_disk_df, parse_loadavg,
    parse_ram_free, parse_ram_vmstat, parse_uptime,
};
use crate::ssh::session::SshSession;

// ---------------------------------------------------------------------------
// Backoff schedule
// ---------------------------------------------------------------------------

const BACKOFF_SECS: [u64; 4] = [30, 60, 120, 300];

struct BackoffState {
    step: usize,
}

impl BackoffState {
    fn new() -> Self {
        Self { step: 0 }
    }

    fn next_delay(&mut self) -> Duration {
        let secs = BACKOFF_SECS[self.step];
        self.step = (self.step + 1).min(BACKOFF_SECS.len() - 1);
        Duration::from_secs(secs)
    }

    fn reset(&mut self) {
        self.step = 0;
    }
}

// ---------------------------------------------------------------------------
// PollManager — owned by App, drives all HostPoller tasks
// ---------------------------------------------------------------------------

/// Manages background metric polling for all hosts.
///
/// Drop this struct to abort all poller tasks.
pub struct PollManager {
    task_handles: Vec<JoinHandle<()>>,
    /// Per-host channel to send an immediate-refresh signal.
    refresh_txs: HashMap<String, mpsc::Sender<()>>,
}

impl PollManager {
    /// Spawn one poller task per host.
    pub fn start(hosts: Vec<Host>, tx: mpsc::Sender<AppEvent>, poll_interval: Duration) -> Self {
        let mut task_handles = Vec::with_capacity(hosts.len());
        let mut refresh_txs = HashMap::with_capacity(hosts.len());

        for host in hosts {
            let (refresh_tx, refresh_rx) = mpsc::channel::<()>(4);
            refresh_txs.insert(host.name.clone(), refresh_tx);

            let event_tx = tx.clone();
            let interval = poll_interval;
            let handle = tokio::spawn(run_host_poller(host, event_tx, interval, refresh_rx));
            task_handles.push(handle);
        }

        Self {
            task_handles,
            refresh_txs,
        }
    }

    /// Trigger an immediate poll for all hosts (called on `r` key press).
    pub fn refresh_all(&self) {
        for (name, tx) in &self.refresh_txs {
            if tx.try_send(()).is_err() {
                tracing::debug!(host = %name, "refresh signal dropped — channel full or closed");
            }
        }
    }

    /// Abort all poller tasks. Called on app exit to allow clean shutdown.
    /// SSH sessions are dropped inside the tasks, which triggers
    /// russh's graceful disconnect.
    pub fn shutdown(self) {
        for handle in &self.task_handles {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Per-host poller task
// ---------------------------------------------------------------------------

async fn run_host_poller(
    host: Host,
    tx: mpsc::Sender<AppEvent>,
    poll_interval: Duration,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    let mut backoff = BackoffState::new();
    let mut session: Option<SshSession> = None;
    let mut discovery_done = false; // Track if we've done Quick Scan

    loop {
        // Ensure we have a live session.
        if session.is_none() {
            send_status(&tx, &host.name, ConnectionStatus::Connecting).await;
            match SshSession::connect(&host).await {
                Ok(s) => {
                    backoff.reset();
                    send_status(&tx, &host.name, ConnectionStatus::Connected).await;
                    session = Some(s);
                    discovery_done = false; // Reset discovery flag on new connection
                }
                Err(e) => {
                    tracing::debug!(host = %host.name, error = %e, "connection failed");
                    send_status(&tx, &host.name, ConnectionStatus::Failed(e.to_string())).await;
                    // Wait with backoff, allowing early refresh.
                    let delay = backoff.next_delay();
                    wait_or_refresh(delay, &mut refresh_rx).await;
                    continue;
                }
            }
        }

        // Run Quick Scan once per connection (don't block UI)
        // This happens right after connection before the first metrics poll
        if !discovery_done {
            if let Some(sess) = &session {
                // Run discovery asynchronously
                // Clone the session since Handle is Arc-based and cheap to clone
                let sess_clone = sess.clone();
                let host_name = host.name.clone();
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    match crate::ssh::discovery::quick_scan(
                        &sess_clone,
                        host_name.clone(),
                        tx_clone.clone(),
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::debug!(host = %host_name, "quick scan completed successfully");
                        }
                        Err(e) => {
                            tracing::warn!(host = %host_name, error = %e, "quick scan failed");
                            let _ = tx_clone
                                .send(AppEvent::DiscoveryFailed(host_name, e.to_string()))
                                .await;
                        }
                    }
                });
                discovery_done = true;
            }
        }

        // Collect metrics using the live session.
        // SAFETY: we only reach this point if session was set to Some above
        // (either just connected or carried over from the previous iteration).
        // This is a single-task async loop with no concurrent mutation, so
        // the expect is always satisfied.
        let sess = session.as_ref().expect("session is Some here");
        match collect_metrics(sess, &host.name).await {
            Ok(metrics) => {
                if tx
                    .send(AppEvent::MetricsUpdate(host.name.clone(), metrics))
                    .await
                    .is_err()
                {
                    break; // App has shut down.
                }
            }
            Err(e) => {
                tracing::debug!(host = %host.name, error = %e, "metric collection failed");
                // Session is broken — drop it and reconnect next iteration.
                session.take();
                send_status(&tx, &host.name, ConnectionStatus::Failed(e.to_string())).await;
                let delay = backoff.next_delay();
                wait_or_refresh(delay, &mut refresh_rx).await;
                continue;
            }
        }

        // Wait for the next poll interval or a manual refresh signal.
        wait_or_refresh(poll_interval, &mut refresh_rx).await;
    }
}

/// Wait for `delay`, but return early if a refresh signal is received.
async fn wait_or_refresh(delay: Duration, refresh_rx: &mut mpsc::Receiver<()>) {
    tokio::select! {
        _ = tokio::time::sleep(delay) => {}
        _ = refresh_rx.recv() => {}
    }
}

async fn send_status(tx: &mpsc::Sender<AppEvent>, name: &str, status: ConnectionStatus) {
    let _ = tx
        .send(AppEvent::HostStatusChanged(name.to_string(), status))
        .await;
}

// ---------------------------------------------------------------------------
// Metric collection
// ---------------------------------------------------------------------------

/// Run all metric commands and return a [`Metrics`] snapshot.
///
/// Tries Linux commands first. If the output doesn't match the expected
/// format, falls back to macOS/BSD variants (graceful degradation per
/// the risk matrix in tech.md §10).
///
/// Returns `Err` when all commands fail simultaneously — this indicates a dead
/// session and should prompt the caller to reconnect.
async fn collect_metrics(session: &SshSession, host_name: &str) -> anyhow::Result<Metrics> {
    // Run all commands concurrently for speed.
    let (cpu_out, mem_out, disk_out, uptime_out, loadavg_out) = tokio::join!(
        session.run_command("top -bn1 2>/dev/null | head -5"),
        session.run_command("free -b 2>/dev/null || vm_stat 2>/dev/null"),
        session.run_command("df -k / 2>/dev/null"),
        session.run_command("uptime 2>/dev/null"),
        session.run_command("cat /proc/loadavg 2>/dev/null"),
    );

    // If every command failed the session is almost certainly dead — return an
    // error so the poller drops the session and reconnects.
    if cpu_out.is_err()
        && mem_out.is_err()
        && disk_out.is_err()
        && uptime_out.is_err()
        && loadavg_out.is_err()
    {
        let err = cpu_out
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("all metric commands failed"));
        return Err(anyhow::anyhow!(
            "all metric commands failed (session may be dead): {}",
            err
        ));
    }

    // Log individual command failures at debug level so operators can distinguish
    // "metric unavailable on this OS" from "command errored".
    let cpu_str = cpu_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "cpu command failed"))
        .unwrap_or_default();
    let mem_str = mem_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "mem command failed"))
        .unwrap_or_default();
    let disk_str = disk_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "disk command failed"))
        .unwrap_or_default();
    let uptime_str = uptime_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "uptime command failed"))
        .unwrap_or_default();
    let loadavg_str = loadavg_out
        .inspect_err(|e| tracing::debug!(host = %host_name, error = %e, "loadavg command failed"))
        .unwrap_or_default();

    let cpu_percent = parse_cpu_combined(&cpu_str, session).await;

    let ram_percent = parse_ram_combined(&mem_str, session).await;

    let disk_percent = parse_disk_df(&disk_str).or_else(|| {
        if !disk_str.is_empty() {
            tracing::debug!(host = %host_name, "disk output present but parse failed");
        }
        None
    });

    let uptime = parse_uptime(&uptime_str);

    let load_avg = parse_loadavg(&loadavg_str);

    Ok(Metrics {
        cpu_percent,
        ram_percent,
        disk_percent,
        uptime,
        load_avg,
        os_info: None, // OS info is collected during discovery, not metrics polling
        last_updated: Instant::now(),
    })
}

async fn parse_cpu_combined(top_out: &str, session: &SshSession) -> Option<f64> {
    // Try Linux top format first.
    if let Some(v) = parse_cpu_top(top_out) {
        return Some(v);
    }
    // Try macOS top format.
    let macos_out = session
        .run_command("top -l 1 -n 0 2>/dev/null | grep 'CPU usage'")
        .await
        .unwrap_or_default();
    if let Some(v) = parse_cpu_top_macos(&macos_out) {
        return Some(v);
    }
    // Fall back to /proc/stat.
    let stat_out = session
        .run_command("head -1 /proc/stat 2>/dev/null")
        .await
        .unwrap_or_default();
    parse_cpu_proc_stat(&stat_out)
}

async fn parse_ram_combined(mem_out: &str, session: &SshSession) -> Option<f64> {
    // Try Linux free -b output.
    if let Some(v) = parse_ram_free(mem_out) {
        return Some(v);
    }
    // vm_stat output (macOS) — also need sysctl hw.memsize.
    if mem_out.contains("Mach Virtual Memory") {
        let memsize_out = session
            .run_command("sysctl hw.memsize 2>/dev/null")
            .await
            .unwrap_or_default();
        return parse_ram_vmstat(mem_out, &memsize_out);
    }
    None
}
