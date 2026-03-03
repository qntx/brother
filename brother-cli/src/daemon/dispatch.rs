//! Command dispatch — routes each [`Request`] to its handler.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::{Request, Response, ResponseData};

use super::{DaemonState, get_page, handlers, page_display, page_eval, page_ok, page_text};

#[allow(clippy::cognitive_complexity, clippy::large_stack_frames)]
pub(super) async fn dispatch(req: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    if let Some(resp) = check_policy(state, &req).await {
        return resp;
    }
    match req {
        Request::Launch {
            headed,
            proxy,
            executable_path,
            user_data_dir,
            extra_args,
            user_agent,
            ignore_https_errors,
            download_path,
            viewport_width,
            viewport_height,
            extensions,
            color_scheme,
            allowed_domains,
        } => {
            dispatch_launch(
                state,
                headed,
                proxy,
                executable_path,
                user_data_dir,
                extra_args,
                user_agent,
                ignore_https_errors,
                download_path,
                viewport_width,
                viewport_height,
                extensions,
                color_scheme,
                allowed_domains,
            )
            .await
        }
        Request::Connect { target } => handlers::cmd_connect(state, &target).await,
        Request::Navigate { url, wait } => handlers::cmd_navigate(state, &url, wait).await,
        Request::Back => page_ok!(state, go_back()),
        Request::Forward => page_ok!(state, go_forward()),
        Request::Reload => page_ok!(state, reload()),
        Request::Snapshot { options } => handlers::cmd_snapshot(state, options).await,
        Request::Screenshot {
            full_page,
            selector,
            format,
            quality,
        } => {
            handlers::cmd_screenshot(state, full_page, selector.as_deref(), &format, quality).await
        }
        Request::Eval { expression } => page_eval!(state, eval(&expression)),
        Request::Click {
            target,
            button,
            click_count,
            delay,
            new_tab,
        } => handlers::cmd_click(state, &target, button, click_count, delay, new_tab).await,
        Request::DblClick { target } => page_ok!(state, &target, dblclick(&target)),
        Request::Fill { target, value } => page_ok!(state, &target, fill(&target, &value)),
        Request::Type {
            target,
            text,
            delay_ms,
            clear,
        } => handlers::cmd_type(state, target.as_deref(), &text, delay_ms, clear).await,
        Request::Press { key } => page_ok!(state, key_press(&key)),
        Request::Select { target, values } => {
            page_ok!(state, &target, select_options(&target, &values))
        }
        Request::Check { target } => page_ok!(state, &target, check(&target)),
        Request::Uncheck { target } => page_ok!(state, &target, uncheck(&target)),
        Request::Hover { target } => page_ok!(state, &target, hover(&target)),
        Request::Focus { target } => page_ok!(state, &target, focus(&target)),
        Request::Scroll {
            direction,
            pixels,
            target,
        } => {
            page_ok!(state, scroll(direction, pixels, target.as_deref()))
        }
        Request::SetValue { target, value } => {
            page_ok!(state, &target, set_value(&target, &value))
        }
        Request::Frame { selector } => handlers::cmd_frame(state, &selector).await,
        Request::MainFrame => handlers::cmd_main_frame(state).await,
        Request::KeyDown { key } => page_ok!(state, key_down(&key)),
        Request::KeyUp { key } => page_ok!(state, key_up(&key)),
        Request::InsertText { text } => page_ok!(state, insert_text(&text)),
        Request::Upload { target, files } => page_ok!(state, &target, upload(&target, &files)),
        Request::Drag { source, target } => page_ok!(state, &source, drag(&source, &target)),
        Request::Clear { target } => page_ok!(state, &target, clear(&target)),
        Request::ScrollIntoView { target } => {
            page_ok!(state, &target, scroll_into_view(&target))
        }
        Request::BoundingBox { target } => handlers::cmd_bounding_box(state, &target).await,
        Request::SetContent { html } => page_ok!(state, set_content(&html)),
        Request::Pdf { path } => page_ok!(state, pdf(&path)),
        Request::Route {
            pattern,
            action,
            status,
            body,
            content_type,
        } => handlers::cmd_route(state, pattern, action, status, body, content_type).await,
        Request::Unroute { pattern } => handlers::cmd_unroute(state, &pattern).await,
        Request::Requests { action, filter } => {
            handlers::cmd_requests(state, action.as_deref(), filter.as_deref()).await
        }
        Request::SetDownloadPath { path } => handlers::cmd_set_download_path(state, &path).await,
        Request::Downloads { action } => handlers::cmd_downloads(state, action.as_deref()).await,
        Request::Download {
            target,
            path,
            timeout_ms,
        } => handlers::cmd_download(state, &target, path.as_deref(), timeout_ms).await,
        Request::WaitForDownload { path, timeout_ms } => {
            handlers::cmd_wait_for_download(state, path.as_deref(), timeout_ms).await
        }
        Request::ResponseBody { url, timeout_ms } => {
            handlers::cmd_response_body(state, &url, timeout_ms).await
        }
        Request::ClipboardRead => page_text!(state, clipboard_read()),
        Request::ClipboardWrite { text } => page_ok!(state, clipboard_write(&text)),
        Request::Find {
            by,
            value,
            name,
            exact,
            subaction,
            fill_value,
        } => {
            handlers::cmd_find(
                state,
                &by,
                &value,
                name.as_deref(),
                exact,
                subaction.as_deref(),
                fill_value.as_deref(),
            )
            .await
        }
        Request::Nth {
            selector,
            index,
            subaction,
            fill_value,
        } => {
            handlers::cmd_nth(
                state,
                &selector,
                index,
                subaction.as_deref(),
                fill_value.as_deref(),
            )
            .await
        }
        Request::Expose { name } => handlers::cmd_expose(state, &name).await,
        Request::DeviceList => handlers::cmd_device_list(),
        Request::WindowNew { width, height } => {
            handlers::cmd_window_new(state, width, height).await
        }
        Request::Device { name } => handlers::cmd_device(state, &name).await,
        Request::Viewport { width, height } => page_ok!(state, set_viewport(width, height)),
        Request::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        } => {
            page_ok!(
                state,
                emulate_media(
                    media.as_deref(),
                    color_scheme.as_deref(),
                    reduced_motion.as_deref(),
                    forced_colors.as_deref(),
                )
            )
        }
        Request::Offline { offline } => page_ok!(state, set_offline(offline)),
        Request::ExtraHeaders { headers_json } => {
            handlers::cmd_extra_headers(state, &headers_json).await
        }
        Request::Geolocation {
            latitude,
            longitude,
            accuracy,
        } => {
            page_ok!(state, set_geolocation(latitude, longitude, accuracy))
        }
        Request::Credentials { username, password } => {
            page_ok!(state, set_credentials(&username, &password))
        }
        Request::UserAgent { user_agent } => page_ok!(state, set_user_agent(&user_agent)),
        Request::Timezone { timezone_id } => page_ok!(state, set_timezone(&timezone_id)),
        Request::Locale { locale } => page_ok!(state, set_locale(&locale)),
        Request::Permissions { permissions, grant } => {
            page_ok!(state, set_permissions(&permissions, grant))
        }
        Request::BringToFront => page_ok!(state, bring_to_front()),
        Request::AddInitScript { script } => page_ok!(state, add_init_script(&script)),
        Request::AddScript { content, url } => {
            page_ok!(state, add_script(content.as_deref(), url.as_deref()))
        }
        Request::AddStyle { content, url } => {
            page_ok!(state, add_style(content.as_deref(), url.as_deref()))
        }
        Request::Dispatch {
            target,
            event,
            event_init,
        } => {
            page_ok!(
                state,
                dispatch_event(&target, &event, event_init.as_deref())
            )
        }
        Request::Styles { target } => page_eval!(state, get_styles(&target)),
        Request::SelectAll { target } => page_ok!(state, select_all_text(&target)),
        Request::Highlight { target } => page_ok!(state, &target, highlight(&target)),
        Request::MouseMove { x, y } => page_ok!(state, mouse_move(x, y)),
        Request::MouseDown { button } => page_ok!(state, mouse_down(button)),
        Request::MouseUp { button } => page_ok!(state, mouse_up(button)),
        Request::Wheel {
            delta_x,
            delta_y,
            selector,
        } => {
            page_ok!(state, wheel(delta_x, delta_y, selector.as_deref()))
        }
        Request::Tap { target } => page_ok!(state, &target, tap(&target)),
        Request::GetText { target } => page_text!(state, get_text(target.as_deref())),
        Request::GetInnerText { target } => page_text!(state, &target, get_inner_text(&target)),
        Request::GetContent => page_text!(state, content()),
        Request::GetUrl => page_text!(state, url()),
        Request::GetTitle => page_text!(state, title()),
        Request::GetHtml { target } => page_text!(state, &target, get_html(&target)),
        Request::GetValue { target } => page_text!(state, &target, get_value(&target)),
        Request::GetAttribute { target, attribute } => {
            page_text!(state, &target, get_attribute(&target, &attribute))
        }
        Request::IsVisible { target } => page_display!(state, &target, is_visible(&target)),
        Request::IsEnabled { target } => page_display!(state, &target, is_enabled(&target)),
        Request::IsChecked { target } => page_display!(state, &target, is_checked(&target)),
        Request::Count { selector } => page_display!(state, &selector, count(&selector)),
        Request::Wait { condition } => handlers::cmd_wait(state, condition).await,
        Request::DialogMessage => handlers::cmd_dialog_message(state).await,
        Request::DialogAccept { prompt_text } => {
            page_ok!(state, dialog_accept(prompt_text.as_deref()))
        }
        Request::DialogDismiss => page_ok!(state, dialog_dismiss()),
        Request::GetCookies => page_eval!(state, get_cookies()),
        Request::SetCookies { cookies } => page_ok!(state, set_cookies(&cookies)),
        Request::SetCookie { cookie } => page_ok!(state, set_cookie(&cookie)),
        Request::ClearCookies => page_ok!(state, clear_cookies()),
        Request::GetStorage { key, session } => page_text!(state, get_storage(&key, session)),
        Request::SetStorage {
            key,
            value,
            session,
        } => {
            page_ok!(state, set_storage(&key, &value, session))
        }
        Request::ClearStorage { session } => page_ok!(state, clear_storage(session)),
        Request::TabNew { url } => handlers::cmd_tab_new(state, url.as_deref()).await,
        Request::TabList => handlers::cmd_tab_list(state).await,
        Request::TabSelect { index } => handlers::cmd_tab_select(state, index).await,
        Request::TabClose { index } => handlers::cmd_tab_close(state, index).await,
        Request::Console { clear } => handlers::cmd_console(state, clear).await,
        Request::Errors { clear } => handlers::cmd_errors(state, clear).await,
        Request::DiffSnapshot { baseline, options } => {
            handlers::cmd_diff_snapshot(state, &baseline, options).await
        }
        Request::DiffScreenshot {
            baseline,
            threshold,
            full_page,
        } => handlers::cmd_diff_screenshot(state, &baseline, threshold, full_page).await,
        Request::DiffUrl {
            url_a,
            url_b,
            screenshot,
            threshold,
            options,
        } => handlers::cmd_diff_url(state, &url_a, &url_b, screenshot, threshold, options).await,
        Request::StateSave { name } => handlers::cmd_state_save(state, &name).await,
        Request::StateLoad { name } => handlers::cmd_state_load(state, &name).await,
        Request::StateList => handlers::cmd_state_list().await,
        Request::StateClear { name } => handlers::cmd_state_clear(&name).await,
        Request::StateShow { name } => handlers::cmd_state_show(&name).await,
        Request::StateClean { days } => handlers::cmd_state_clean(days).await,
        Request::StateRename { old_name, new_name } => {
            handlers::cmd_state_rename(&old_name, &new_name).await
        }
        Request::TraceStart { categories } => handlers::cmd_trace_start(state, &categories).await,
        Request::TraceStop { path } => handlers::cmd_trace_stop(state, path.as_deref()).await,
        Request::ProfilerStart { categories } => {
            handlers::cmd_profiler_start(state, &categories).await
        }
        Request::ProfilerStop { path } => handlers::cmd_profiler_stop(state, path.as_deref()).await,
        Request::ScreencastStart {
            format,
            quality,
            max_width,
            max_height,
        } => handlers::cmd_screencast_start(state, &format, quality, max_width, max_height).await,
        Request::ScreencastStop => handlers::cmd_screencast_stop(state).await,
        Request::HarStart => handlers::cmd_har_start(state).await,
        Request::HarStop { path } => handlers::cmd_har_stop(state, path.as_deref()).await,
        Request::SetAllowedDomains { domains } => {
            handlers::cmd_set_allowed_domains(state, domains).await
        }
        Request::Status => handlers::cmd_status(state).await,
        Request::Close => Response::ok(),
    }
}

async fn check_policy(state: &Arc<Mutex<DaemonState>>, req: &Request) -> Option<Response> {
    let mut guard = state.lock().await;
    let cache = guard.policy_cache.as_mut()?;
    let cmd_name = request_cmd_name(req);
    let policy = cache.get();
    if super::policy::check_policy(&cmd_name, policy) == super::policy::PolicyDecision::Deny {
        let category = super::policy::get_category(&cmd_name).unwrap_or("unknown");
        return Some(Response::error(format!(
            "action denied by policy: command '{cmd_name}' (category '{category}') is not allowed"
        )));
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_launch(
    state: &Arc<Mutex<DaemonState>>,
    headed: bool,
    proxy: Option<String>,
    executable_path: Option<String>,
    user_data_dir: Option<String>,
    extra_args: Vec<String>,
    user_agent: Option<String>,
    ignore_https_errors: bool,
    download_path: Option<String>,
    viewport_width: u32,
    viewport_height: u32,
    extensions: Vec<String>,
    color_scheme: Option<String>,
    allowed_domains: Vec<String>,
) -> Response {
    let mut guard = state.lock().await;
    if guard.browser.is_some() {
        return Response::ok();
    }
    let mut config = brother::BrowserConfig::default()
        .headless(!headed)
        .ignore_https_errors(ignore_https_errors)
        .viewport(viewport_width, viewport_height);
    if let Some(p) = proxy {
        config = config.proxy(p);
    }
    if let Some(ep) = executable_path {
        config = config.executable(ep);
    }
    if let Some(ud) = user_data_dir {
        config = config.user_data_dir(ud);
    }
    if let Some(ua) = user_agent {
        config = config.user_agent(ua);
    }
    if let Some(dp) = download_path {
        config = config.download_path(dp);
    }
    for ext in &extensions {
        config.args.push(format!("--load-extension={ext}"));
    }
    config.args.extend(extra_args);
    guard.pending_color_scheme = color_scheme;
    if !allowed_domains.is_empty() {
        guard.allowed_domains = allowed_domains;
    }
    guard.launch_config = Some(config);
    Response::ok()
}

fn request_cmd_name(req: &Request) -> String {
    serde_json::to_value(req)
        .ok()
        .and_then(|v| v.get("cmd").and_then(|c| c.as_str().map(String::from)))
        .unwrap_or_else(|| "unknown".to_owned())
}
