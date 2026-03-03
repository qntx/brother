//! Command handlers — dispatched from [`super::dispatch`].
//!
//! Each submodule groups related handlers by domain:
//! - [`navigation`]: Navigate, connect, frame management.
//! - [`observation`]: Snapshot, screenshot, wait, bounding box, console, errors, status.
//! - [`interaction`]: Click, type, find, nth, expose, extra headers.
//! - [`network`]: Route, unroute, requests, downloads, HAR, response body.
//! - [`tabs`]: Tab/window create, list, select, close.
//! - [`emulation`]: Device presets, screencast.
//! - [`diff`]: Snapshot diff, screenshot diff, URL diff.
//! - [`state`]: State persistence (save/load/list/clear/show/clean/rename).
//! - [`trace`]: CDP tracing, profiler, domain filter.

mod diff;
mod emulation;
mod interaction;
mod navigation;
mod network;
mod observation;
mod state;
mod tabs;
mod trace;

pub(super) use diff::{cmd_diff_screenshot, cmd_diff_snapshot, cmd_diff_url};
pub(super) use emulation::{cmd_device, cmd_device_list, cmd_screencast_start, cmd_screencast_stop};
pub(super) use interaction::{
    cmd_click, cmd_expose, cmd_extra_headers, cmd_find, cmd_nth, cmd_type,
};
pub(super) use navigation::{cmd_connect, cmd_frame, cmd_main_frame, cmd_navigate};
pub(super) use network::{
    cmd_download, cmd_downloads, cmd_har_start, cmd_har_stop, cmd_requests, cmd_response_body,
    cmd_route, cmd_set_download_path, cmd_unroute, cmd_wait_for_download,
};
pub(super) use observation::{
    cmd_bounding_box, cmd_console, cmd_dialog_message, cmd_errors, cmd_screenshot, cmd_snapshot,
    cmd_status, cmd_wait,
};
pub(super) use state::{
    cmd_state_clean, cmd_state_clear, cmd_state_list, cmd_state_load, cmd_state_rename,
    cmd_state_save, cmd_state_show,
};
pub(super) use tabs::{cmd_tab_close, cmd_tab_list, cmd_tab_new, cmd_tab_select, cmd_window_new};
pub(super) use trace::{
    cmd_profiler_start, cmd_profiler_stop, cmd_set_allowed_domains, cmd_trace_start, cmd_trace_stop,
};
