//! External integration tests for metric parsers.
//!
//! Tests use realistic command outputs captured from Ubuntu 22.04, CentOS 7,
//! Alpine Linux (BusyBox), and macOS 14. All parsers must return `None` on
//! empty or malformed input without panicking.

use omnyssh::ssh::metrics::{
    parse_cpu_proc_stat, parse_cpu_top, parse_cpu_top_macos, parse_disk_df, parse_loadavg,
    parse_ram_free, parse_ram_vmstat, parse_uptime, threshold_color,
};
use ratatui::style::Color;

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

#[test]
fn cpu_ubuntu_procps_ng() {
    let out = "%Cpu(s):  2.3 us,  0.7 sy,  0.0 ni, 96.7 id,  0.3 wa,  0.0 hi,  0.0 si,  0.0 st";
    let pct = parse_cpu_top(out).expect("parse ubuntu cpu");
    assert!((pct - 3.3).abs() < 0.2, "expected ~3.3, got {pct}");
}

#[test]
fn cpu_centos7_old_format() {
    let out = "Cpu(s):  2.3%us,  0.7%sy,  0.0%ni, 96.7%id,  0.3%wa,  0.0%hi,  0.0%si,  0.0%st";
    let pct = parse_cpu_top(out).expect("parse centos7 cpu");
    assert!((pct - 3.3).abs() < 0.2, "expected ~3.3, got {pct}");
}

#[test]
fn cpu_alpine_busybox() {
    let out = "CPU:   4% usr   1% sys   0% nic  94% idle   0% io   0% irq   1% sirq";
    let pct = parse_cpu_top(out).expect("parse alpine cpu");
    assert!((pct - 6.0).abs() < 0.2, "expected ~6.0, got {pct}");
}

#[test]
fn cpu_macos_top() {
    let out = "CPU usage: 3.17% user, 1.56% sys, 95.26% idle";
    let pct = parse_cpu_top_macos(out).expect("parse macos cpu");
    assert!((pct - 4.74).abs() < 0.2, "expected ~4.74, got {pct}");
}

#[test]
fn cpu_macos_top_full_output() {
    let out = "Processes: 412 total, 2 running, 410 sleeping, 2178 threads \n\
               2024/01/15 14:23:05\n\
               Load Avg: 1.52, 1.74, 1.89\n\
               CPU usage: 5.71% user, 2.57% sys, 91.71% idle\n\
               SharedLibs: 438M resident";
    let pct = parse_cpu_top_macos(out).expect("parse macos full cpu");
    assert!((pct - 8.29).abs() < 0.2, "expected ~8.29, got {pct}");
}

#[test]
fn cpu_proc_stat() {
    // Realistic /proc/stat line from a lightly loaded Ubuntu server.
    let out = "cpu  74608 2520 24433 1117073 6176 4054 0 0 0 0";
    let pct = parse_cpu_proc_stat(out).expect("parse proc/stat");
    assert!(pct > 0.0 && pct < 20.0, "expected ~10%, got {pct}");
}

#[test]
fn cpu_idle_zero_gives_100_percent() {
    let out = "%Cpu(s): 100.0 us,  0.0 sy,  0.0 ni,  0.0 id,  0.0 wa,  0.0 hi,  0.0 si,  0.0 st";
    let pct = parse_cpu_top(out).expect("parse fully-loaded cpu");
    assert!((pct - 100.0).abs() < 0.5, "expected 100, got {pct}");
}

#[test]
fn cpu_empty_returns_none() {
    assert!(parse_cpu_top("").is_none());
    assert!(parse_cpu_top("no cpu data here").is_none());
    assert!(parse_cpu_top_macos("").is_none());
    assert!(parse_cpu_proc_stat("").is_none());
}

#[test]
fn cpu_partial_output_no_panic() {
    // Simulates truncated SSH output.
    assert!(parse_cpu_top("%Cpu(s):  2.3 us,  0.7 sy").is_none());
}

// ---------------------------------------------------------------------------
// RAM
// ---------------------------------------------------------------------------

#[test]
fn ram_free_modern_available_column() {
    // 8 GB total, ~4.9 GB available → ~40% used
    let out = "              total        used        free      shared  buff/cache   available\n\
               Mem:    8192000000  3145728000   512000000   134217728  4534272000  4915200000\n\
               Swap:   2147483648           0  2147483648";
    let pct = parse_ram_free(out).expect("parse modern free -b");
    assert!((pct - 40.0).abs() < 2.0, "expected ~40%, got {pct}");
}

#[test]
fn ram_free_busybox_3col() {
    // Alpine busybox free — 3 data columns only.
    let out = "              total        used        free\n\
               Mem:       1018736      524288      494448";
    let pct = parse_ram_free(out).expect("parse busybox free");
    // (1018736 - 494448) / 1018736 ≈ 51.5%
    assert!((pct - 51.5).abs() < 2.0, "expected ~51.5%, got {pct}");
}

#[test]
fn ram_free_empty_returns_none() {
    assert!(parse_ram_free("").is_none());
    assert!(parse_ram_free("only the header line\n").is_none());
}

#[test]
fn ram_vmstat_macos() {
    let vm_stat = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                   Pages free:                               23456.\n\
                   Pages active:                           456789.\n\
                   Pages inactive:                         123456.\n\
                   Pages speculative:                        12345.\n\
                   Pages throttled:                              0.\n\
                   Pages wired down:                        98765.\n";
    let memsize = "hw.memsize: 17179869184";
    let pct = parse_ram_vmstat(vm_stat, memsize).expect("parse vm_stat");
    assert!(pct > 0.0 && pct <= 100.0, "pct out of range: {pct}");
}

#[test]
fn ram_vmstat_empty_returns_none() {
    assert!(parse_ram_vmstat("", "").is_none());
    assert!(parse_ram_vmstat("", "hw.memsize: 0").is_none());
}

// ---------------------------------------------------------------------------
// Disk
// ---------------------------------------------------------------------------

#[test]
fn disk_df_linux_19pct() {
    let out = "Filesystem     1K-blocks    Used Available Use% Mounted on\n\
               /dev/sda1       51475068 9000000  39841436  19% /";
    let pct = parse_disk_df(out).expect("parse linux df");
    assert!((pct - 19.0).abs() < 0.5, "expected 19%, got {pct}");
}

#[test]
fn disk_df_macos_capacity() {
    let out =
        "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
               /dev/disk3s5   994662584 516879368 400765064    57% 5488234 4293478045    0%   /";
    let pct = parse_disk_df(out).expect("parse macos df");
    assert!((pct - 57.0).abs() < 0.5, "expected 57%, got {pct}");
}

#[test]
fn disk_df_100pct() {
    let out = "Filesystem     1K-blocks    Used Available Use% Mounted on\n\
               /dev/sda1       51475068 51475068          0 100% /";
    let pct = parse_disk_df(out).expect("parse full disk");
    assert!((pct - 100.0).abs() < 0.5, "expected 100%, got {pct}");
}

#[test]
fn disk_df_empty_returns_none() {
    assert!(parse_disk_df("").is_none());
    assert!(parse_disk_df("Filesystem 1K-blocks Used Available Use% Mounted on\n").is_none());
}

// ---------------------------------------------------------------------------
// Uptime
// ---------------------------------------------------------------------------

#[test]
fn uptime_linux_days_and_hours() {
    let out = " 14:23:45 up 2 days,  3:45,  2 users,  load average: 0.15, 0.10, 0.08";
    let result = parse_uptime(out).expect("parse linux uptime");
    // When days are present, only show "2 days" without the time
    assert_eq!(result, "2 days", "got: {result}");
}

#[test]
fn uptime_linux_hours_only() {
    let out = " 10:00:00 up  3:45,  1 user,  load average: 0.00, 0.01, 0.05";
    let result = parse_uptime(out).expect("parse linux uptime hours");
    assert!(result.contains("3:45"), "got: {result}");
}

#[test]
fn uptime_empty_returns_none() {
    assert!(parse_uptime("").is_none());
}

// ---------------------------------------------------------------------------
// Load average
// ---------------------------------------------------------------------------

#[test]
fn loadavg_linux() {
    let out = "0.15 0.10 0.08 1/423 12345\n";
    let result = parse_loadavg(out).expect("parse loadavg");
    assert_eq!(result, "0.15 0.10 0.08");
}

#[test]
fn loadavg_empty_returns_none() {
    assert!(parse_loadavg("").is_none());
}

// ---------------------------------------------------------------------------
// Threshold colour
// ---------------------------------------------------------------------------

#[test]
fn threshold_green_below_60() {
    assert_eq!(threshold_color(0.0), Color::Green);
    assert_eq!(threshold_color(59.9), Color::Green);
}

#[test]
fn threshold_yellow_60_to_85() {
    assert_eq!(threshold_color(60.0), Color::Yellow);
    assert_eq!(threshold_color(75.0), Color::Yellow);
    assert_eq!(threshold_color(85.0), Color::Yellow);
}

#[test]
fn threshold_red_above_85() {
    assert_eq!(threshold_color(85.1), Color::Red);
    assert_eq!(threshold_color(100.0), Color::Red);
}
