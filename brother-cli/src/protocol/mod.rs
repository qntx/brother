//! JSON protocol for daemon ↔ CLI communication.
//!
//! The daemon listens on `127.0.0.1:<port>`. Each message is a single JSON
//! object terminated by `\n`. The CLI sends a [`Request`]; the daemon replies
//! with a [`Response`].

pub mod paths;
pub mod request;
pub mod response;
pub mod types;

pub use paths::{pid_file_path_for, port_file_path_for, runtime_dir};
pub use request::Request;
pub use response::{Response, ResponseData};
pub use types::{RouteAction, WaitCondition, WaitStrategy};
