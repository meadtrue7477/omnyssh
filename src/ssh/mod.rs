/// SSH client, session management, SFTP and metrics collection.
///
/// Supports system SSH for interactive sessions, native russh client for
/// metrics collection, PTY-backed multi-session terminal emulator,
/// Smart Server Context with service discovery, and Auto SSH Key Setup
/// for secure authentication.
pub mod client;
pub mod discovery;
pub mod key_setup;
pub mod metrics;
pub mod pool;
pub mod probe;
pub mod pty;
pub mod services;
pub mod session;
pub mod sftp;
