//! Map CLI [`Command`] to daemon [`Request`].

use brother::{MouseButton, ScrollDirection};

use crate::commands::Command;
use crate::protocol::{Request, RouteAction, WaitCondition, WaitStrategy};

/// Map CLI subcommand to daemon protocol request.
pub fn build_request(cmd: &Command) -> Request {
    match cmd {
        Command::Connect { target } => Request::Connect {
            target: target.clone(),
        },
        Command::Open { url } => Request::Navigate {
            url: normalize_url(url),
            wait: WaitStrategy::Load,
        },
        Command::Snapshot {
            interactive,
            compact,
            depth,
            selector,
            cursor,
        } => {
            let mut opts = brother::SnapshotOptions::default()
                .interactive_only(*interactive)
                .compact(*compact)
                .max_depth(*depth)
                .cursor_interactive(*cursor);
            if let Some(sel) = selector {
                opts = opts.selector(sel.clone());
            }
            Request::Snapshot { options: opts }
        }
        Command::Click {
            target,
            button,
            click_count,
            delay,
            new_tab,
        } => Request::Click {
            target: target.clone(),
            button: parse_mouse_button(button),
            click_count: *click_count,
            delay: *delay,
            new_tab: *new_tab,
        },
        Command::Dblclick { target } => Request::DblClick {
            target: target.clone(),
        },
        Command::Fill { target, value } => Request::Fill {
            target: target.clone(),
            value: value.clone(),
        },
        Command::Type {
            text,
            target,
            delay,
            clear,
        } => Request::Type {
            target: target.clone(),
            text: text.clone(),
            delay_ms: *delay,
            clear: *clear,
        },
        Command::Press { key } => Request::Press { key: key.clone() },
        Command::Select { target, values } => Request::Select {
            target: target.clone(),
            values: values.clone(),
        },
        Command::Check { target } => Request::Check {
            target: target.clone(),
        },
        Command::Uncheck { target } => Request::Uncheck {
            target: target.clone(),
        },
        Command::Hover { target } => Request::Hover {
            target: target.clone(),
        },
        Command::Focus { target } => Request::Focus {
            target: target.clone(),
        },
        Command::Scroll {
            direction,
            pixels,
            target,
        } => Request::Scroll {
            direction: parse_direction(direction),
            pixels: *pixels,
            target: target.clone(),
        },
        Command::Frame { selector } => Request::Frame {
            selector: selector.clone(),
        },
        Command::MainFrame => Request::MainFrame,
        Command::KeyDown { key } => Request::KeyDown { key: key.clone() },
        Command::KeyUp { key } => Request::KeyUp { key: key.clone() },
        Command::InsertText { text } => Request::InsertText { text: text.clone() },
        Command::Upload { target, files } => Request::Upload {
            target: target.clone(),
            files: files.clone(),
        },
        Command::Drag { source, target } => Request::Drag {
            source: source.clone(),
            target: target.clone(),
        },
        Command::Clear { target } => Request::Clear {
            target: target.clone(),
        },
        Command::ScrollIntoView { target } => Request::ScrollIntoView {
            target: target.clone(),
        },
        Command::BoundingBox { target } => Request::BoundingBox {
            target: target.clone(),
        },
        Command::SetContent { html } => Request::SetContent { html: html.clone() },
        Command::Pdf { path } => Request::Pdf { path: path.clone() },
        Command::Screenshot {
            full_page,
            selector,
            format,
            quality,
            ..
        } => Request::Screenshot {
            full_page: *full_page,
            selector: selector.clone(),
            format: format.clone(),
            quality: *quality,
        },
        Command::Eval { expression } => Request::Eval {
            expression: expression.clone(),
        },
        Command::Get { what, target, attr } => {
            build_get_request(what, target.as_deref(), attr.as_deref())
        }
        Command::Back => Request::Back,
        Command::Forward => Request::Forward,
        Command::Reload => Request::Reload,
        Command::Wait {
            target,
            text,
            url,
            load,
            function,
            timeout,
        } => build_wait_request(
            target.as_deref(),
            text.as_deref(),
            url.as_deref(),
            load.as_deref(),
            function.as_deref(),
            *timeout,
        ),
        Command::Find {
            by,
            value,
            name,
            exact,
        } => Request::Find {
            by: by.clone(),
            value: value.clone(),
            name: name.clone(),
            exact: *exact,
        },
        Command::Device { name } => Request::Device { name: name.clone() },
        Command::Viewport { width, height } => Request::Viewport {
            width: *width,
            height: *height,
        },
        Command::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        } => Request::EmulateMedia {
            media: media.clone(),
            color_scheme: color_scheme.clone(),
            reduced_motion: reduced_motion.clone(),
            forced_colors: forced_colors.clone(),
        },
        Command::Offline { offline } => Request::Offline { offline: *offline },
        Command::ExtraHeaders { headers_json } => Request::ExtraHeaders {
            headers_json: headers_json.clone(),
        },
        Command::Geolocation {
            latitude,
            longitude,
            accuracy,
        } => Request::Geolocation {
            latitude: *latitude,
            longitude: *longitude,
            accuracy: *accuracy,
        },
        Command::Credentials { username, password } => Request::Credentials {
            username: username.clone(),
            password: password.clone(),
        },
        Command::UserAgent { user_agent } => Request::UserAgent {
            user_agent: user_agent.clone(),
        },
        Command::Timezone { timezone_id } => Request::Timezone {
            timezone_id: timezone_id.clone(),
        },
        Command::Locale { locale } => Request::Locale {
            locale: locale.clone(),
        },
        Command::Permissions { permissions, deny } => Request::Permissions {
            permissions: permissions.clone(),
            grant: !deny,
        },
        Command::BringToFront => Request::BringToFront,
        Command::Styles { target } => Request::Styles {
            target: target.clone(),
        },
        Command::SelectAll { target } => Request::SelectAll {
            target: target.clone(),
        },
        Command::Highlight { target } => Request::Highlight {
            target: target.clone(),
        },
        Command::MouseMove { x, y } => Request::MouseMove { x: *x, y: *y },
        Command::MouseDown { button } => Request::MouseDown {
            button: parse_mouse_button(button),
        },
        Command::MouseUp { button } => Request::MouseUp {
            button: parse_mouse_button(button),
        },
        Command::Wheel {
            delta_y,
            delta_x,
            selector,
        } => Request::Wheel {
            delta_x: *delta_x,
            delta_y: *delta_y,
            selector: selector.clone(),
        },
        Command::Tap { target } => Request::Tap {
            target: target.clone(),
        },
        Command::SetValue { target, value } => Request::SetValue {
            target: target.clone(),
            value: value.clone(),
        },
        Command::AddInitScript { script } => Request::AddInitScript {
            script: script.clone(),
        },
        Command::AddScript { content, url } => Request::AddScript {
            content: content.clone(),
            url: url.clone(),
        },
        Command::AddStyle { content, url } => Request::AddStyle {
            content: content.clone(),
            url: url.clone(),
        },
        Command::Dispatch {
            target,
            event,
            init,
        } => Request::Dispatch {
            target: target.clone(),
            event: event.clone(),
            event_init: init.clone(),
        },
        Command::ClipboardRead => Request::ClipboardRead,
        Command::ClipboardWrite { text } => Request::ClipboardWrite { text: text.clone() },
        Command::SetDownloadPath { path } => Request::SetDownloadPath { path: path.clone() },
        Command::Downloads { action } => Request::Downloads {
            action: action.clone(),
        },
        Command::DownloadClick {
            target,
            path,
            timeout,
        } => Request::Download {
            target: target.clone(),
            path: path.clone(),
            timeout_ms: *timeout,
        },
        Command::WaitForDownload { path, timeout } => Request::WaitForDownload {
            path: path.clone(),
            timeout_ms: *timeout,
        },
        Command::ResponseBody { url, timeout } => Request::ResponseBody {
            url: url.clone(),
            timeout_ms: *timeout,
        },
        Command::Route {
            pattern,
            action,
            status,
            body,
            content_type,
        } => Request::Route {
            pattern: pattern.clone(),
            action: parse_route_action(action),
            status: *status,
            body: body.clone(),
            content_type: content_type.clone(),
        },
        Command::Unroute { pattern } => Request::Unroute {
            pattern: pattern.clone(),
        },
        Command::Requests { action, filter } => Request::Requests {
            action: action.clone(),
            filter: filter.clone(),
        },
        Command::Dialog { action, text } => match action.as_str() {
            "accept" => Request::DialogAccept {
                prompt_text: text.clone(),
            },
            "dismiss" => Request::DialogDismiss,
            // "message" and any unknown variant default to DialogMessage
            _ => Request::DialogMessage,
        },
        Command::Cookie { action, value } => match action.as_str() {
            "set" => Request::SetCookie {
                cookie: value.clone().unwrap_or_default(),
            },
            "clear" => Request::ClearCookies,
            // "get" and any unknown variant default to GetCookies
            _ => Request::GetCookies,
        },
        Command::Storage {
            action,
            key,
            value,
            session,
        } => match action.as_str() {
            "set" => Request::SetStorage {
                key: key.clone().unwrap_or_default(),
                value: value.clone().unwrap_or_default(),
                session: *session,
            },
            "clear" => Request::ClearStorage { session: *session },
            // "get" and any unknown variant default to GetStorage
            _ => Request::GetStorage {
                key: key.clone().unwrap_or_default(),
                session: *session,
            },
        },
        Command::StateCheck { what, target } | Command::Is { what, target } => {
            match what.as_str() {
                "enabled" => Request::IsEnabled {
                    target: target.clone(),
                },
                "checked" => Request::IsChecked {
                    target: target.clone(),
                },
                "count" => Request::Count {
                    selector: target.clone(),
                },
                // "visible" and any unknown variant default to IsVisible
                _ => Request::IsVisible {
                    target: target.clone(),
                },
            }
        }
        Command::TabNew { url } => Request::TabNew { url: url.clone() },
        Command::TabList => Request::TabList,
        Command::TabSelect { index } => Request::TabSelect { index: *index },
        Command::TabClose { index } => Request::TabClose { index: *index },
        Command::Console { clear } => Request::Console { clear: *clear },
        Command::Errors { clear } => Request::Errors { clear: *clear },
        Command::DiffSnapshot { sub } => match sub {
            crate::commands::DiffSub::Snapshot {
                baseline,
                interactive,
                compact,
            } => {
                let baseline_text = baseline
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                let opts = brother::SnapshotOptions::default()
                    .interactive_only(*interactive)
                    .compact(*compact);
                Request::DiffSnapshot {
                    baseline: baseline_text,
                    options: opts,
                }
            }
            crate::commands::DiffSub::Screenshot {
                baseline,
                threshold,
                full_page,
            } => {
                let baseline_b64 = std::fs::read(baseline)
                    .map(|bytes| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes))
                    .unwrap_or_default();
                Request::DiffScreenshot {
                    baseline: baseline_b64,
                    threshold: *threshold,
                    full_page: *full_page,
                }
            }
            crate::commands::DiffSub::Url {
                url_a,
                url_b,
                screenshot,
                threshold,
            } => Request::DiffUrl {
                url_a: url_a.clone(),
                url_b: url_b.clone(),
                screenshot: *screenshot,
                threshold: *threshold,
            },
        },
        Command::State(sub) => match sub {
            crate::commands::StateSub::Save { name } => Request::StateSave { name: name.clone() },
            crate::commands::StateSub::Load { name } => Request::StateLoad { name: name.clone() },
            crate::commands::StateSub::List => Request::StateList,
            crate::commands::StateSub::Clear { name } => Request::StateClear { name: name.clone() },
            crate::commands::StateSub::Show { name } => Request::StateShow { name: name.clone() },
            crate::commands::StateSub::Clean { days } => Request::StateClean { days: *days },
            crate::commands::StateSub::Rename { old_name, new_name } => Request::StateRename {
                old_name: old_name.clone(),
                new_name: new_name.clone(),
            },
        },
        Command::Trace {
            action,
            categories,
            output,
        } => match action.as_str() {
            "stop" => Request::TraceStop {
                path: output.clone(),
            },
            _ => Request::TraceStart {
                categories: categories
                    .as_deref()
                    .map(|c| c.split(',').map(|s| s.trim().to_owned()).collect())
                    .unwrap_or_default(),
            },
        },
        Command::Profiler {
            action,
            categories,
            output,
        } => match action.as_str() {
            "stop" => Request::ProfilerStop {
                path: output.clone(),
            },
            _ => Request::ProfilerStart {
                categories: categories
                    .as_deref()
                    .map(|c| c.split(',').map(|s| s.trim().to_owned()).collect())
                    .unwrap_or_default(),
            },
        },
        Command::AllowedDomains { domains } => Request::SetAllowedDomains {
            domains: domains.clone(),
        },
        Command::Status | Command::Daemon => Request::Status,
        Command::Close => Request::Close,
    }
}

fn parse_direction(s: &str) -> ScrollDirection {
    match s.to_ascii_lowercase().as_str() {
        "up" => ScrollDirection::Up,
        "left" => ScrollDirection::Left,
        "right" => ScrollDirection::Right,
        _ => ScrollDirection::Down,
    }
}

fn parse_mouse_button(s: &str) -> MouseButton {
    match s.to_ascii_lowercase().as_str() {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

fn parse_route_action(s: &str) -> RouteAction {
    match s.to_ascii_lowercase().as_str() {
        "fulfill" => RouteAction::Fulfill,
        _ => RouteAction::Abort,
    }
}

fn build_get_request(what: &str, target: Option<&str>, attr: Option<&str>) -> Request {
    match what {
        "url" => Request::GetUrl,
        "title" => Request::GetTitle,
        "content" => Request::GetContent,
        "innertext" => Request::GetInnerText {
            target: target.unwrap_or("body").to_owned(),
        },
        "html" => Request::GetHtml {
            target: target.unwrap_or("body").to_owned(),
        },
        "value" => Request::GetValue {
            target: target.unwrap_or("input").to_owned(),
        },
        "attribute" | "attr" => Request::GetAttribute {
            target: target.unwrap_or("body").to_owned(),
            attribute: attr.unwrap_or("class").to_owned(),
        },
        // Default: get text
        _ => Request::GetText {
            target: target.map(str::to_owned),
        },
    }
}

#[allow(clippy::option_if_let_else)] // Explicit priority chain is clearer than nested map_or_else.
fn build_wait_request(
    target: Option<&str>,
    text: Option<&str>,
    url: Option<&str>,
    load: Option<&str>,
    function: Option<&str>,
    timeout: u64,
) -> Request {
    // Priority: explicit flags first, then positional target
    let condition = if let Some(t) = text {
        WaitCondition::Text {
            text: t.to_owned(),
            timeout_ms: timeout,
        }
    } else if let Some(u) = url {
        WaitCondition::Url {
            pattern: u.to_owned(),
            timeout_ms: timeout,
        }
    } else if let Some(f) = function {
        WaitCondition::Function {
            expression: f.to_owned(),
            timeout_ms: timeout,
        }
    } else if let Some(l) = load {
        let state = match l {
            "domcontentloaded" => WaitStrategy::DomContentLoaded,
            "networkidle" => WaitStrategy::NetworkIdle,
            _ => WaitStrategy::Load,
        };
        WaitCondition::LoadState {
            state,
            timeout_ms: timeout,
        }
    } else if let Some(sel) = target {
        // Numeric → duration; otherwise → CSS selector
        sel.parse::<u64>().map_or_else(
            |_| WaitCondition::Selector {
                selector: sel.to_owned(),
                timeout_ms: timeout,
            },
            |ms| WaitCondition::Duration { ms },
        )
    } else {
        WaitCondition::Duration { ms: timeout }
    };
    Request::Wait { condition }
}

/// Auto-prepend `https://` if the URL has no scheme.
fn normalize_url(url: &str) -> String {
    if url.contains("://") || url.starts_with("data:") || url.starts_with("about:") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}
