//! Command dispatch — routes each [`Request`] to its handler.

use std::sync::Arc;

use base64::Engine;
use tokio::sync::Mutex;

use crate::protocol::{Request, Response, ResponseData};

use super::{get_page, page_eval, page_ok, page_text, DaemonState};

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::large_stack_frames
)]
pub(super) async fn dispatch(req: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    match req {
        // -- Launch / Connection ----------------------------------------------
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
        } => {
            let mut guard = state.lock().await;
            if guard.browser.is_some() {
                // Browser already running — ignore Launch, just return ok.
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
            config.args = extra_args;
            guard.launch_config = Some(config);
            Response::ok()
        }

        Request::Connect { target } => super::handlers::cmd_connect(state, &target).await,

        // -- Navigation -------------------------------------------------------
        Request::Navigate { url, wait } => {
            super::handlers::cmd_navigate(state, &url, wait).await
        }
        Request::Back => page_ok!(state, go_back()),
        Request::Forward => page_ok!(state, go_forward()),
        Request::Reload => page_ok!(state, reload()),

        // -- Observation ------------------------------------------------------
        Request::Snapshot { options } => super::handlers::cmd_snapshot(state, options).await,
        Request::Screenshot {
            full_page,
            selector,
            format,
            quality,
        } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page
                .screenshot(full_page, selector.as_deref(), &format, Some(quality))
                .await
            {
                Ok(bytes) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    Response::ok_data(ResponseData::Screenshot { data })
                }
                Err(e) => Response::error(format!("screenshot failed: {e}")),
            }
        }
        Request::Eval { expression } => page_eval!(state, eval(&expression)),

        // -- Interaction ------------------------------------------------------
        Request::Click {
            target,
            button,
            click_count,
            delay,
            new_tab,
        } => {
            if new_tab {
                // Ctrl+click to open in new tab, then switch to it.
                let page = match get_page(state).await {
                    Ok(p) => p,
                    Err(r) => return r,
                };
                if let Err(e) = page.key_down("Control").await {
                    return Response::error(e.to_string());
                }
                let click_result = page.click(&target).await;
                let _ = page.key_up("Control").await;
                if let Err(e) = click_result {
                    return Response::error(e.ai_friendly(&target).to_string());
                }
                // Wait briefly for the new tab to appear, then switch.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let mut guard = state.lock().await;
                if let Some(ref browser) = guard.browser {
                    if let Ok(pages) = browser.pages().await {
                        for p in pages {
                            let url = p.url().await.unwrap_or_default();
                            if !guard.pages.iter().any(|ep| {
                                futures::executor::block_on(ep.url()).unwrap_or_default() == url
                            }) {
                                guard.pages.push(p);
                            }
                        }
                    }
                    guard.active_tab = guard.pages.len().saturating_sub(1);
                }
                Response::ok()
            } else {
                page_ok!(
                    state,
                    &target,
                    click_with(&target, button, click_count, delay)
                )
            }
        }
        Request::DblClick { target } => page_ok!(state, &target, dblclick(&target)),
        Request::Fill { target, value } => page_ok!(state, &target, fill(&target, &value)),
        Request::Type {
            target,
            text,
            delay_ms,
            clear,
        } => {
            if clear {
                if let Some(ref t) = target {
                    page_ok!(state, t, fill(t, &text))
                } else {
                    page_ok!(state, type_with_delay(None, &text, delay_ms))
                }
            } else {
                page_ok!(state, type_with_delay(target.as_deref(), &text, delay_ms))
            }
        }
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

        // -- Frame (iframe) ---------------------------------------------------
        Request::Frame { selector } => super::handlers::cmd_frame(state, &selector).await,
        Request::MainFrame => super::handlers::cmd_main_frame(state).await,

        // -- Raw keyboard -----------------------------------------------------
        Request::KeyDown { key } => page_ok!(state, key_down(&key)),
        Request::KeyUp { key } => page_ok!(state, key_up(&key)),
        Request::InsertText { text } => page_ok!(state, insert_text(&text)),

        // -- File / DOM -------------------------------------------------------
        Request::Upload { target, files } => page_ok!(state, &target, upload(&target, &files)),
        Request::Drag { source, target } => page_ok!(state, &source, drag(&source, &target)),
        Request::Clear { target } => page_ok!(state, &target, clear(&target)),
        Request::ScrollIntoView { target } => {
            page_ok!(state, &target, scroll_into_view(&target))
        }
        Request::BoundingBox { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.bounding_box(&target).await {
                Ok((x, y, w, h)) => Response::ok_data(ResponseData::BoundingBox {
                    x,
                    y,
                    width: w,
                    height: h,
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::SetContent { html } => page_ok!(state, set_content(&html)),
        Request::Pdf { path } => page_ok!(state, pdf(&path)),

        // -- Network interception ---------------------------------------------
        Request::Route {
            pattern,
            action,
            status,
            body,
            content_type,
        } => super::handlers::cmd_route(state, pattern, action, status, body, content_type).await,
        Request::Unroute { pattern } => super::handlers::cmd_unroute(state, &pattern).await,
        Request::Requests { action, filter } => {
            super::handlers::cmd_requests(state, action.as_deref(), filter.as_deref()).await
        }

        // -- Download ---------------------------------------------------------
        Request::SetDownloadPath { path } => {
            super::handlers::cmd_set_download_path(state, &path).await
        }
        Request::Downloads { action } => {
            super::handlers::cmd_downloads(state, action.as_deref()).await
        }
        Request::Download {
            target,
            path,
            timeout_ms,
        } => {
            super::handlers::cmd_download(state, &target, path.as_deref(), timeout_ms).await
        }
        Request::WaitForDownload { path, timeout_ms } => {
            super::handlers::cmd_wait_for_download(state, path.as_deref(), timeout_ms).await
        }
        Request::ResponseBody { url, timeout_ms } => {
            super::handlers::cmd_response_body(state, &url, timeout_ms).await
        }

        // -- Clipboard --------------------------------------------------------
        Request::ClipboardRead => page_text!(state, clipboard_read()),
        Request::ClipboardWrite { text } => page_ok!(state, clipboard_write(&text)),

        // -- Semantic locators -------------------------------------------------
        Request::Find {
            by,
            value,
            name,
            exact,
        } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let result = match by.as_str() {
                "role" => page.find_by_role(&value, name.as_deref()).await,
                "text" => page.find_by_text(&value, exact).await,
                "label" => page.find_by_label(&value).await,
                "placeholder" => page.find_by_placeholder(&value).await,
                "testid" => page.find_by_testid(&value).await,
                "alttext" | "alt" => page.find_by_alt_text(&value, exact).await,
                "title" => page.find_by_title(&value, exact).await,
                _ => {
                    return Response::error(format!(
                        "unknown locator type '{by}'. Use: role, text, label, placeholder, testid, alttext, title"
                    ))
                }
            };
            match result {
                Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
                Err(e) => Response::error(e.to_string()),
            }
        }

        // -- Environment emulation --------------------------------------------
        Request::Device { name } => {
            let Some(preset) = brother::DevicePreset::lookup(&name) else {
                let names = brother::DevicePreset::list_names().join(", ");
                return Response::error(format!(
                    "unknown device '{name}'. Available: {names}"
                ));
            };
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            if let Err(e) = page.set_viewport(preset.width, preset.height).await {
                return Response::error(e.to_string());
            }
            if let Err(e) = page.set_user_agent(preset.user_agent).await {
                return Response::error(e.to_string());
            }
            Response::ok_data(ResponseData::Text {
                text: format!(
                    "emulating {} ({}x{}, {})",
                    preset.name, preset.width, preset.height, preset.user_agent
                ),
            })
        }
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
            let map: serde_json::Map<String, serde_json::Value> =
                match serde_json::from_str(&headers_json) {
                    Ok(m) => m,
                    Err(e) => return Response::error(format!("invalid headers JSON: {e}")),
                };
            page_ok!(state, set_extra_headers(map))
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

        // -- Script injection -------------------------------------------------
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

        // -- Misc interaction / queries ---------------------------------------
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

        // -- Query ------------------------------------------------------------
        Request::GetText { target } => page_text!(state, get_text(target.as_deref())),
        Request::GetInnerText { target } => {
            page_text!(state, &target, get_inner_text(&target))
        }
        Request::GetContent => page_text!(state, content()),
        Request::GetUrl => page_text!(state, url()),
        Request::GetTitle => page_text!(state, title()),
        Request::GetHtml { target } => page_text!(state, &target, get_html(&target)),
        Request::GetValue { target } => page_text!(state, &target, get_value(&target)),
        Request::GetAttribute { target, attribute } => {
            page_text!(state, &target, get_attribute(&target, &attribute))
        }

        // -- State checks -----------------------------------------------------
        Request::IsVisible { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_visible(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::IsEnabled { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_enabled(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::IsChecked { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_checked(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::Count { selector } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.count(&selector).await {
                Ok(n) => Response::ok_data(ResponseData::Text {
                    text: n.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&selector).to_string()),
            }
        }

        // -- Wait -------------------------------------------------------------
        Request::Wait { condition } => super::handlers::cmd_wait(state, condition).await,

        // -- Dialog -----------------------------------------------------------
        Request::DialogMessage => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            page.dialog_message().await.map_or_else(
                || {
                    Response::ok_data(ResponseData::Text {
                        text: "(no dialog)".into(),
                    })
                },
                |info| {
                    let value = serde_json::to_value(&info).unwrap_or_default();
                    Response::ok_data(ResponseData::Eval { value })
                },
            )
        }
        Request::DialogAccept { prompt_text } => {
            page_ok!(state, dialog_accept(prompt_text.as_deref()))
        }
        Request::DialogDismiss => page_ok!(state, dialog_dismiss()),

        // -- Cookie / Storage -------------------------------------------------
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

        // -- Tab management ---------------------------------------------------
        Request::TabNew { url } => super::handlers::cmd_tab_new(state, url.as_deref()).await,
        Request::TabList => super::handlers::cmd_tab_list(state).await,
        Request::TabSelect { index } => super::handlers::cmd_tab_select(state, index).await,
        Request::TabClose { index } => super::handlers::cmd_tab_close(state, index).await,

        // -- Debug ------------------------------------------------------------
        Request::Console { clear } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let logs = page.take_console_logs().await;
            if clear {
                return Response::ok_data(ResponseData::Text {
                    text: format!("{} console entries cleared", logs.len()),
                });
            }
            Response::ok_data(ResponseData::Logs {
                entries: serde_json::to_value(&logs).unwrap_or_default(),
            })
        }
        Request::Errors { clear } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            let errors = page.take_js_errors().await;
            if clear {
                return Response::ok_data(ResponseData::Text {
                    text: format!("{} error entries cleared", errors.len()),
                });
            }
            Response::ok_data(ResponseData::Logs {
                entries: serde_json::to_value(&errors).unwrap_or_default(),
            })
        }

        // -- Diff -------------------------------------------------------------
        Request::DiffSnapshot { baseline, options } => {
            super::handlers::cmd_diff_snapshot(state, &baseline, options).await
        }
        Request::DiffScreenshot {
            baseline,
            threshold,
            full_page,
        } => super::handlers::cmd_diff_screenshot(state, &baseline, threshold, full_page).await,

        Request::DiffUrl {
            url_a,
            url_b,
            screenshot,
            threshold,
        } => super::handlers::cmd_diff_url(state, &url_a, &url_b, screenshot, threshold).await,

        // -- State persistence ------------------------------------------------
        Request::StateSave { name } => super::handlers::cmd_state_save(state, &name).await,
        Request::StateLoad { name } => super::handlers::cmd_state_load(state, &name).await,
        Request::StateList => super::handlers::cmd_state_list().await,
        Request::StateClear { name } => super::handlers::cmd_state_clear(&name).await,
        Request::StateShow { name } => super::handlers::cmd_state_show(&name).await,
        Request::StateClean { days } => super::handlers::cmd_state_clean(days).await,
        Request::StateRename { old_name, new_name } => {
            super::handlers::cmd_state_rename(&old_name, &new_name).await
        }

        // -- Debug / Tracing --------------------------------------------------
        Request::TraceStart { categories } => {
            super::handlers::cmd_trace_start(state, &categories).await
        }
        Request::TraceStop { path } => {
            super::handlers::cmd_trace_stop(state, path.as_deref()).await
        }
        Request::ProfilerStart { categories } => {
            super::handlers::cmd_profiler_start(state, &categories).await
        }
        Request::ProfilerStop { path } => {
            super::handlers::cmd_profiler_stop(state, path.as_deref()).await
        }

        // -- Security ---------------------------------------------------------
        Request::SetAllowedDomains { domains } => {
            super::handlers::cmd_set_allowed_domains(state, domains).await
        }

        // -- Lifecycle --------------------------------------------------------
        Request::Status => {
            let guard = state.lock().await;
            let browser_running = guard.browser.is_some();
            let page_url = if let Some(page) = guard.pages.get(guard.active_tab) {
                page.url().await.ok()
            } else {
                None
            };
            Response::ok_data(ResponseData::Status {
                browser_running,
                page_url,
            })
        }
        Request::Close => Response::ok(),
    }
}
