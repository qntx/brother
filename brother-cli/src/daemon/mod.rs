//! Daemon server — long-running process that holds the browser instance.
//!
//! Listens on `127.0.0.1:<port>`, accepts newline-delimited JSON
//! [`Request`](crate::protocol::Request) messages, and returns
//! [`Response`](crate::protocol::Response) messages. The browser is lazily
//! launched on first command.

mod auth;
mod debug;
mod diff;
mod dispatch;
mod emulate;
mod interact;
#[macro_use]
pub mod macros;
mod navigate;
mod network;
mod observe;
mod persist;
mod server;
pub mod state;
mod tab;

pub use server::run_session;
