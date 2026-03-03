//! Map CLI [`Command`] to daemon [`Request`].

use brother::{MouseButton, ScrollDirection};

use crate::commands::{
    AuthSub, ClipboardSub, Command, CookieSub, DialogSub, DiffSub, HarSub, InputSub, MouseSub,
    ProfilerSub, ScreencastSub, StateSub, StorageSub, TabSub, TraceSub,
};
use crate::protocol::{Request, RouteAction, WaitCondition, WaitStrategy};

/// Map CLI subcommand to daemon protocol request (consumes the command).
pub fn build_request(cmd: Command) -> Request {
    match cmd {
        Command::Connect { target } => Request::Connect { target },
        Command::Open { url, headers } => Request::Navigate {
            url: normalize_url(&url),
            wait: WaitStrategy::Load,
            headers: parse_header_list(&headers),
        },
        Command::Snapshot {
            interactive,
            compact,
            depth,
            selector,
            cursor,
        } => {
            let mut opts = brother::SnapshotOptions::default()
                .interactive_only(interactive)
                .compact(compact)
                .max_depth(depth)
                .cursor_interactive(cursor);
            if let Some(sel) = selector {
                opts = opts.selector(sel);
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
            target,
            button: parse_mouse_button(&button),
            click_count,
            delay,
            new_tab,
        },
        Command::Dblclick { target } => Request::DblClick { target },
        Command::Fill { target, value } => Request::Fill { target, value },
        Command::Type {
            text,
            target,
            delay,
            clear,
        } => Request::Type {
            target,
            text,
            delay_ms: delay,
            clear,
        },
        Command::Press { key } => Request::Press { key },
        Command::Select { target, values } => Request::Select { target, values },
        Command::Check { target } => Request::Check { target },
        Command::Uncheck { target } => Request::Uncheck { target },
        Command::Hover { target } => Request::Hover { target },
        Command::Focus { target } => Request::Focus { target },
        Command::Scroll {
            direction,
            pixels,
            target,
        } => Request::Scroll {
            direction: parse_direction(&direction),
            pixels,
            target,
        },
        Command::Frame { selector } => Request::Frame { selector },
        Command::MainFrame => Request::MainFrame,
        Command::KeyDown { key } => Request::KeyDown { key },
        Command::KeyUp { key } => Request::KeyUp { key },
        Command::InsertText { text } => Request::InsertText { text },
        Command::Upload { target, files } => Request::Upload { target, files },
        Command::Drag { source, target } => Request::Drag { source, target },
        Command::Clear { target } => Request::Clear { target },
        Command::ScrollIntoView { target } => Request::ScrollIntoView { target },
        Command::BoundingBox { target } => Request::BoundingBox { target },
        Command::SetContent { html } => Request::SetContent { html },
        Command::Pdf { path, format } => Request::Pdf {
            path,
            paper_format: format,
        },
        Command::Screenshot {
            full_page,
            selector,
            format,
            quality,
            annotate,
            ..
        } => Request::Screenshot {
            full_page,
            selector,
            format,
            quality,
            annotate,
        },
        Command::Eval { expression } => Request::Eval { expression },
        Command::Get { what, target, attr } => build_get_request(&what, target, attr),
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
        } => build_wait_request(target, text, url, load, function, timeout),
        Command::Find {
            by,
            value,
            name,
            exact,
            subaction,
            fill_value,
        } => Request::Find {
            by,
            value,
            name,
            exact,
            subaction,
            fill_value,
        },
        Command::Device { name } => Request::Device { name },
        Command::DeviceList => Request::DeviceList,
        Command::WindowNew { width, height } => Request::WindowNew { width, height },
        Command::Viewport { width, height } => Request::Viewport { width, height },
        Command::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        } => Request::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        },
        Command::Offline { offline } => Request::Offline { offline },
        Command::ExtraHeaders { headers_json } => Request::ExtraHeaders { headers_json },
        Command::Geolocation {
            latitude,
            longitude,
            accuracy,
        } => Request::Geolocation {
            latitude,
            longitude,
            accuracy,
        },
        Command::Credentials { username, password } => Request::Credentials { username, password },
        Command::UserAgent { user_agent } => Request::UserAgent { user_agent },
        Command::Timezone { timezone_id } => Request::Timezone { timezone_id },
        Command::Locale { locale } => Request::Locale { locale },
        Command::Permissions { permissions, deny } => Request::Permissions {
            permissions,
            grant: !deny,
        },
        Command::BringToFront => Request::BringToFront,
        Command::Styles { target } => Request::Styles { target },
        Command::SelectAll { target } => Request::SelectAll { target },
        Command::Highlight { target } => Request::Highlight { target },
        Command::Mouse(sub) => match sub {
            MouseSub::Move { x, y } => Request::MouseMove { x, y },
            MouseSub::Down { button } => Request::MouseDown {
                button: parse_mouse_button(&button),
            },
            MouseSub::Up { button } => Request::MouseUp {
                button: parse_mouse_button(&button),
            },
        },
        Command::Wheel {
            delta_y,
            delta_x,
            selector,
        } => Request::Wheel {
            delta_x,
            delta_y,
            selector,
        },
        Command::Tap { target } => Request::Tap { target },
        Command::SetValue { target, value } => Request::SetValue { target, value },
        Command::AddInitScript { script } => Request::AddInitScript { script },
        Command::AddScript { content, url } => Request::AddScript { content, url },
        Command::AddStyle { content, url } => Request::AddStyle { content, url },
        Command::Dispatch {
            target,
            event,
            init,
        } => Request::Dispatch {
            target,
            event,
            event_init: init,
        },
        Command::Clipboard(sub) => match sub {
            ClipboardSub::Read => Request::ClipboardRead,
            ClipboardSub::Write { text } => Request::ClipboardWrite { text },
        },
        Command::SetDownloadPath { path } => Request::SetDownloadPath { path },
        Command::Downloads { action } => Request::Downloads { action },
        Command::DownloadClick {
            target,
            path,
            timeout,
        } => Request::Download {
            target,
            path,
            timeout_ms: timeout,
        },
        Command::WaitForDownload { path, timeout } => Request::WaitForDownload {
            path,
            timeout_ms: timeout,
        },
        Command::ResponseBody { url, timeout } => Request::ResponseBody {
            url,
            timeout_ms: timeout,
        },
        Command::Route {
            pattern,
            action,
            status,
            body,
            content_type,
        } => Request::Route {
            pattern,
            action: parse_route_action(&action),
            status,
            body,
            content_type,
        },
        Command::Unroute { pattern } => Request::Unroute { pattern },
        Command::Requests { action, filter } => Request::Requests { action, filter },
        Command::Dialog(sub) => match sub {
            DialogSub::Message => Request::DialogMessage,
            DialogSub::Accept { text } => Request::DialogAccept { prompt_text: text },
            DialogSub::Dismiss => Request::DialogDismiss,
        },
        Command::Cookie(sub) => match sub {
            CookieSub::Get => Request::GetCookies,
            CookieSub::Set { value } => Request::SetCookie { cookie: value },
            CookieSub::Clear => Request::ClearCookies,
        },
        Command::Storage(sub) => match sub {
            StorageSub::Get { key, session } => Request::GetStorage { key, session },
            StorageSub::Set {
                key,
                value,
                session,
            } => Request::SetStorage {
                key,
                value,
                session,
            },
            StorageSub::Clear { session } => Request::ClearStorage { session },
        },
        Command::Query { what, target } => build_query_request(&what, target),
        Command::Nth {
            selector,
            index,
            subaction,
            fill_value,
        } => Request::Nth {
            selector,
            index,
            subaction,
            fill_value,
        },
        Command::Expose { name } => Request::Expose { name },
        Command::Tab(sub) => match sub {
            TabSub::New { url } => Request::TabNew { url },
            TabSub::List => Request::TabList,
            TabSub::Select { index } => Request::TabSelect { index },
            TabSub::Close { index } => Request::TabClose { index },
        },
        Command::Console { clear } => Request::Console { clear },
        Command::Errors { clear } => Request::Errors { clear },
        Command::DiffSnapshot { sub } => match sub {
            DiffSub::Snapshot {
                baseline,
                interactive,
                compact,
            } => {
                let baseline_text = baseline
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                Request::DiffSnapshot {
                    baseline: baseline_text,
                    options: brother::SnapshotOptions::default()
                        .interactive_only(interactive)
                        .compact(compact),
                }
            }
            DiffSub::Screenshot {
                baseline,
                threshold,
                full_page,
            } => {
                let baseline_b64 = std::fs::read(&baseline)
                    .map(|bytes| {
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)
                    })
                    .unwrap_or_default();
                Request::DiffScreenshot {
                    baseline: baseline_b64,
                    threshold,
                    full_page,
                }
            }
            DiffSub::Url {
                url_a,
                url_b,
                screenshot,
                threshold,
                interactive,
                compact,
                depth,
                selector,
            } => {
                let mut opts = brother::SnapshotOptions::default()
                    .interactive_only(interactive)
                    .compact(compact)
                    .max_depth(depth.unwrap_or(0));
                if let Some(sel) = selector {
                    opts = opts.selector(sel);
                }
                Request::DiffUrl {
                    url_a,
                    url_b,
                    screenshot,
                    threshold,
                    options: opts,
                }
            }
        },
        Command::State(sub) => match sub {
            StateSub::Save { name } => Request::StateSave { name },
            StateSub::Load { name } => Request::StateLoad { name },
            StateSub::List => Request::StateList,
            StateSub::Clear { name } => Request::StateClear { name },
            StateSub::Show { name } => Request::StateShow { name },
            StateSub::Clean { days } => Request::StateClean { days },
            StateSub::Rename { old_name, new_name } => Request::StateRename { old_name, new_name },
        },
        Command::Trace(sub) => match sub {
            TraceSub::Start { categories } => Request::TraceStart {
                categories: parse_categories(categories),
            },
            TraceSub::Stop { output } => Request::TraceStop { path: output },
        },
        Command::Profiler(sub) => match sub {
            ProfilerSub::Start { categories } => Request::ProfilerStart {
                categories: parse_categories(categories),
            },
            ProfilerSub::Stop { output } => Request::ProfilerStop { path: output },
        },
        Command::Screencast(sub) => match sub {
            ScreencastSub::Start {
                format,
                quality,
                max_width,
                max_height,
            } => Request::ScreencastStart {
                format,
                quality,
                max_width,
                max_height,
            },
            ScreencastSub::Stop => Request::ScreencastStop,
        },
        Command::Har(sub) => match sub {
            HarSub::Start => Request::HarStart,
            HarSub::Stop { output } => Request::HarStop { path: output },
        },
        Command::AllowedDomains { domains } => Request::SetAllowedDomains { domains },
        Command::Confirm { id } => Request::Confirm {
            confirmation_id: id,
        },
        Command::DenyAction { id } => Request::Deny {
            confirmation_id: id,
        },
        Command::Input(sub) => match sub {
            InputSub::Mouse {
                event_type,
                x,
                y,
                button,
                click_count,
                delta_x,
                delta_y,
                modifiers,
            } => Request::InputMouse {
                event_type,
                x,
                y,
                button,
                click_count,
                delta_x,
                delta_y,
                modifiers,
            },
            InputSub::Keyboard {
                event_type,
                key,
                code,
                text,
                modifiers,
            } => Request::InputKeyboard {
                event_type,
                key,
                code,
                text,
                modifiers,
            },
            InputSub::Touch {
                event_type,
                points,
                modifiers,
            } => Request::InputTouch {
                event_type,
                touch_points: parse_touch_points(points.as_deref()),
                modifiers,
            },
        },
        Command::Auth(sub) => match sub {
            AuthSub::Save {
                name,
                url,
                username,
                password,
                username_selector,
                password_selector,
                submit_selector,
            } => Request::AuthSave {
                name,
                url,
                username,
                password,
                username_selector,
                password_selector,
                submit_selector,
            },
            AuthSub::Login { name } => Request::AuthLogin { name },
            AuthSub::List => Request::AuthList,
            AuthSub::Delete { name } => Request::AuthDelete { name },
            AuthSub::Show { name } => Request::AuthShow { name },
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

fn parse_categories(input: Option<String>) -> Vec<String> {
    input
        .map(|c| c.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default()
}

fn build_get_request(what: &str, target: Option<String>, attr: Option<String>) -> Request {
    match what {
        "url" => Request::GetUrl,
        "title" => Request::GetTitle,
        "content" => Request::GetContent,
        "innertext" => Request::GetInnerText {
            target: target.unwrap_or_else(|| "body".to_owned()),
        },
        "html" => Request::GetHtml {
            target: target.unwrap_or_else(|| "body".to_owned()),
        },
        "value" => Request::GetValue {
            target: target.unwrap_or_else(|| "input".to_owned()),
        },
        "attribute" | "attr" => Request::GetAttribute {
            target: target.unwrap_or_else(|| "body".to_owned()),
            attribute: attr.unwrap_or_else(|| "class".to_owned()),
        },
        _ => Request::GetText { target },
    }
}

fn build_query_request(what: &str, target: String) -> Request {
    match what {
        "enabled" => Request::IsEnabled { target },
        "checked" => Request::IsChecked { target },
        "count" => Request::Count { selector: target },
        _ => Request::IsVisible { target },
    }
}

fn build_wait_request(
    target: Option<String>,
    text: Option<String>,
    url: Option<String>,
    load: Option<String>,
    function: Option<String>,
    timeout: u64,
) -> Request {
    let condition = match (text, url, function, load, target) {
        (Some(text), ..) => WaitCondition::Text {
            text,
            timeout_ms: timeout,
        },
        (_, Some(pattern), ..) => WaitCondition::Url {
            pattern,
            timeout_ms: timeout,
        },
        (_, _, Some(expression), ..) => WaitCondition::Function {
            expression,
            timeout_ms: timeout,
        },
        (_, _, _, Some(l), _) => WaitCondition::LoadState {
            state: match l.as_str() {
                "domcontentloaded" => WaitStrategy::DomContentLoaded,
                "networkidle" => WaitStrategy::NetworkIdle,
                _ => WaitStrategy::Load,
            },
            timeout_ms: timeout,
        },
        (_, _, _, _, Some(sel)) => sel.parse::<u64>().map_or_else(
            |_| WaitCondition::Selector {
                selector: sel,
                timeout_ms: timeout,
            },
            |ms| WaitCondition::Duration { ms },
        ),
        _ => WaitCondition::Duration { ms: timeout },
    };
    Request::Wait { condition }
}

fn parse_header_list(headers: &[String]) -> std::collections::HashMap<String, String> {
    headers
        .iter()
        .filter_map(|h| {
            let (key, value) = h.split_once(':')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn parse_touch_points(input: Option<&str>) -> Vec<(f64, f64)> {
    let Some(s) = input else {
        return Vec::new();
    };
    serde_json::from_str(s).unwrap_or_default()
}

fn normalize_url(url: &str) -> String {
    if url.contains("://") || url.starts_with("data:") || url.starts_with("about:") {
        url.to_owned()
    } else {
        format!("https://{url}")
    }
}
