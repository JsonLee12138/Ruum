//! Left Icon Rail and full-page navigation views.
//!
//! Provides the primary navigation rail and the full-page views:
//! Sources, Watchdog, and Settings.

use std::time::Duration;

use gpui_kit::base::CheckboxState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, Context, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Rems,
    StatefulInteractiveElement as _, Styled, Svg, Transformation, Window, div,
    radians, svg,
};
use gpui_omarchy::{
    ActiveTheme, ButtonVariant, IconName, button, checkbox, dialog_button,
    dialog_description, dialog_popup, dialog_title, with_tooltip,
};
use ruum_core::removal::RemovalMode;
use ruum_core::size::human_bytes;
use ruum_core::watchdog::ServiceStatus;
use ruum_core::watchdog_config::AiCliProvider;

use crate::state::{CleanupConfirmModal, Ruum, Screen};
use crate::ui::{icon, size, space, text};

const ZAP_SVG: &[u8] = br#"<svg
  xmlns="http://www.w3.org/2000/svg"
  width="24"
  height="24"
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
>
  <path d="M15.914 4a1.5 1.5 0 00-2.474-1.561l-9 9A1.5 1.5 0 005.5 14h4.002a.5.5 0 01.471.666L8.086 20a1.5 1.5 0 002.475 1.56l9-9A1.5 1.5 0 0018.5 10h-3.997a.5.5 0 01-.472-.667z" />
</svg>"#;

fn zap_icon(color: Hsla) -> Svg {
    svg()
        .data(ZAP_SVG)
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
}

fn spinning_loader(id: impl Into<ElementId>, color: Hsla) -> impl IntoElement {
    svg()
        .path(IconName::LoaderCircle.path())
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
        .with_animation(
            id,
            Animation::new(Duration::from_millis(900)).repeat(),
            |this, phase| {
                this.with_transformation(Transformation::rotate(radians(
                    phase * std::f32::consts::TAU,
                )))
            },
        )
}

/// Render a single navigation icon button in the Left Icon Rail.
fn rail_button(
    id: &'static str,
    icon_name: IconName,
    tooltip_text: &'static str,
    active: bool,
    on_click: impl Fn(&mut Ruum, &mut Window, &mut Context<'_, Ruum>) + 'static,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let bg_color = if active {
        theme.accent.opacity(0.18)
    } else {
        theme.accent.opacity(0.0)
    };
    let fg_color = if active {
        theme.bright
    } else {
        theme.foreground.opacity(0.65)
    };
    let border_color = if active {
        theme.accent.opacity(0.4)
    } else {
        theme.accent.opacity(0.0)
    };

    with_tooltip(
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .items_center()
            .justify_center()
            .size(Rems(2.25))
            .rounded(space::XS)
            .bg(bg_color)
            .border_1()
            .border_color(border_color)
            .text_color(fg_color)
            .cursor_pointer()
            .hover(|this| this.bg(theme.accent.opacity(0.1)))
            .child(gpui_omarchy::icon(icon_name))
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
            })),
        tooltip_text,
    )
}

/// Global left navigation rail.
pub fn icon_rail(app: &Ruum, cx: &Context<'_, Ruum>) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let is_explore = app.screen == Screen::Explore && !app.volumes_open;
    let is_sources = app.screen == Screen::Sources || app.volumes_open;
    let is_watchdog = app.screen == Screen::Watchdog;
    let is_settings = app.screen == Screen::Settings;

    div()
        .id("icon-rail")
        .debug_selector(|| "icon-rail".into())
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .w(size::ICON_RAIL)
        .h_full()
        .py(space::MD)
        .bg(theme.background)
        .border_r_1()
        .border_color(theme.divider())
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(space::MD)
                .child(rail_button(
                    "rail-explore",
                    IconName::LayoutDashboard,
                    "Explore treemap",
                    is_explore,
                    |this, _, cx| {
                        this.screen = Screen::Explore;
                        this.volumes_open = false;
                        cx.notify();
                    },
                    cx,
                ))
                .child(rail_button(
                    "rail-sources",
                    IconName::HardDrive,
                    "Sources & Drives · v",
                    is_sources,
                    |this, _, cx| {
                        this.open_volumes(cx);
                    },
                    cx,
                ))
                .child(rail_button(
                    "rail-watchdog",
                    IconName::Eye,
                    "Watchdog",
                    is_watchdog,
                    |this, _, cx| {
                        this.screen = Screen::Watchdog;
                        this.volumes_open = false;
                        cx.notify();
                    },
                    cx,
                )),
        )
        .child(div().flex().flex_col().items_center().child(rail_button(
            "rail-settings",
            IconName::Settings,
            if cfg!(target_os = "macos") {
                "Settings · ⌘,"
            } else {
                "Settings · Ctrl+,"
            },
            is_settings,
            |this, _, cx| {
                this.screen = Screen::Settings;
                this.volumes_open = false;
                cx.notify();
            },
            cx,
        )))
}

/// Full-page Sources and Mounts view migrated from `volumes_dialog`.
pub fn sources_page(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let source_ready = app.picker_source_matches(cx);
    let source = app.volumes_source.as_ref().map_or_else(
        || "This computer".to_owned(),
        |host| format!("{host} · SSH · read-only"),
    );

    let mut rows = div()
        .id("volume-rows")
        .debug_selector(|| "volume-rows".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .max_h(size::SOURCE_LIST_HEIGHT)
        .overflow_y_scroll();

    if app.volumes_loading {
        rows = rows.child(dialog_description(
            "Listing mounts, not scanning files…",
            cx,
        ));
    } else if let Some(error) = &app.volumes_error {
        rows = rows.child(div().text_color(theme.danger).child(error.clone()));
    } else if app.volumes.is_empty() {
        rows = rows.child(dialog_description(
            "No mounts listed. Enter a directory below.",
            cx,
        ));
    }

    for (index, volume) in app.volumes.iter().enumerate() {
        let highlighted = index == app.volume_highlight;
        let label = format!(
            "{}  ·  {}",
            volume.point.display(),
            volume.device.as_deref().unwrap_or("Mounted volume")
        );
        rows = rows.child(
            div()
                .id(ElementId::Name(format!("volume-{index}").into()))
                .debug_selector(move || format!("volume-{index}"))
                .px(space::MD)
                .py(space::SM)
                .text_size(text::BODY)
                .text_color(if highlighted {
                    theme.bright
                } else {
                    theme.foreground
                })
                .when(highlighted, |row| row.bg(theme.accent.opacity(0.18)))
                .hover(|row| row.bg(theme.accent.opacity(0.1)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.volume_highlight = index;
                    window.focus(&this.confirm_focus, cx);
                    cx.notify();
                })),
        );
    }

    let host = app
        .picker_host
        .as_ref()
        .expect("picker prepared before rendering");
    let path = app
        .picker_path
        .as_ref()
        .expect("picker prepared before rendering");

    let card = dialog_popup(cx)
        .w_full()
        .child(dialog_title("Choose what to scan", cx))
        .child(dialog_description(
            "Connect, then select a mount or directory. No new scan starts \
             until you choose Scan.",
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(space::SM)
                .child(
                    button(
                        "source-local",
                        "This computer",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.choose_local_source(window, cx);
                        },
                    )),
                )
                .child(
                    gpui_omarchy::input("source-host", host, window, cx)
                        .debug_selector(|| "source-host".into())
                        .flex_1(),
                )
                .child(
                    button(
                        "source-connect",
                        "Connect SSH",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, _, cx| {
                            this.connect_picker(cx);
                        },
                    )),
                ),
        )
        .child(div().text_color(theme.bright).child(source))
        .child(rows)
        .child(dialog_description(
            "Nested mounts and symlinks are not traversed. Space is measured \
             only after selection.",
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(space::SM)
                .child(
                    gpui_omarchy::input("source-path", path, window, cx)
                        .debug_selector(|| "source-path".into())
                        .flex_1(),
                )
                .child(
                    button(
                        "source-scan-path",
                        "Scan directory",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .disabled(
                        !source_ready
                            || app.volumes_loading
                            || app.volumes_error.is_some(),
                    )
                    .on_click(cx.listener(
                        |this, _, _, cx| {
                            this.choose_picker_path(cx);
                        },
                    )),
                ),
        )
        .children(
            app.picker_error.as_ref().map(|error| {
                div().text_color(theme.danger).child(error.clone())
            }),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::SM)
                .child(
                    dialog_button(
                        "source-cancel",
                        "Cancel",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.close_volumes(cx);
                            this.apply_focus(window, cx);
                        },
                    )),
                )
                .child(
                    dialog_button(
                        "source-scan",
                        "Scan selected root",
                        ButtonVariant::Primary,
                        cx,
                    )
                    .disabled(
                        !source_ready
                            || app.volumes_loading
                            || app.volumes_error.is_some()
                            || app.volumes.is_empty(),
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.choose_volume(cx);
                            this.apply_focus(window, cx);
                        },
                    )),
                ),
        );

    let mut outer = div()
        .id("sources-page")
        .debug_selector(|| "sources-page".into())
        .track_focus(&app.confirm_focus)
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .p(space::XL)
        .overflow_y_scroll()
        .on_click(cx.listener(|this, _, window, cx| {
            this.close_volumes(cx);
            this.apply_focus(window, cx);
        }));

    if app.awaiting_root {
        outer = outer.child(
            div()
                .debug_selector(|| "choose-root-idle".into())
                .flex()
                .flex_col()
                .items_center()
                .gap(space::SM)
                .mb(space::LG)
                .child(
                    div()
                        .text_size(text::HEADING)
                        .child("Choose a source. Then choose a root."),
                ),
        );
    }

    let rail_px = size::ICON_RAIL.0 * app.rem;
    let available_w =
        (window.viewport_size().width.as_f32() - rail_px).max(0.0);
    let available_h = window.viewport_size().height.as_f32();

    outer.child(
        div()
            .id("source-picker")
            .debug_selector(|| "source-picker".into())
            .w(size::SOURCE_PICKER)
            .max_w(Rems(available_w / app.rem * size::SOURCE_VIEWPORT_FRACTION))
            .max_h(Rems(available_h / app.rem * size::SOURCE_VIEWPORT_FRACTION))
            .overflow_y_scroll()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(card),
    )
}

/// Watchdog monitor and recommendation page.
/// Format a unix-seconds check timestamp as a concise human-readable age.
fn format_checked_age(checked_secs: Option<u64>) -> String {
    let Some(checked) = checked_secs else {
        return "Not checked yet".to_string();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let delta = now.saturating_sub(checked);
    if delta < 5 {
        "Just now".to_string()
    } else if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

/// Watchdog monitor status, manual trigger, and proposal history.
pub fn watchdog_page(
    app: &Ruum,
    _window: &Window,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let home = std::env::home_dir();
    let home_display = home
        .as_ref()
        .map_or_else(|| "/".to_string(), |h| h.display().to_string());

    let (free_str, total_str, free_bytes) = if let Some(space) = &app.space {
        (
            human_bytes(space.available),
            human_bytes(space.total),
            space.available,
        )
    } else {
        ("Unknown".to_string(), "Unknown".to_string(), 0)
    };

    let thresh_bytes = app.watchdog_config.min_free_gb * 1024 * 1024 * 1024;
    let is_breached = free_bytes < thresh_bytes;

    let service_label = match app.watchdog_service_status {
        Some(ServiceStatus::Active) => "Active (Running)",
        Some(ServiceStatus::Inactive) => "Inactive (Stopped)",
        Some(ServiceStatus::NotInstalled) => "Not Installed",
        Some(ServiceStatus::Unsupported) | None => "Not Registered",
    };
    let service_color = match app.watchdog_service_status {
        Some(ServiceStatus::Active) => theme.accent,
        Some(
            ServiceStatus::Inactive
            | ServiceStatus::NotInstalled
            | ServiceStatus::Unsupported,
        )
        | None => theme.secondary,
    };

    let status_card = dialog_popup(cx)
        .w_full()
        .child(dialog_title("Volume Status", cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::SM)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Monitored Root:"),
                        )
                        .child(
                            div().text_color(theme.bright).child(home_display),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Free Space:"),
                        )
                        .child(
                            div()
                                .text_color(if is_breached {
                                    theme.danger
                                } else {
                                    theme.bright
                                })
                                .child(format!("{free_str} of {total_str}")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Alert Threshold:"),
                        )
                        .child(div().text_color(theme.bright).child(format!(
                            "{} GB",
                            app.watchdog_config.min_free_gb
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div().text_color(theme.secondary).child("Status:"),
                        )
                        .child(
                            div()
                                .text_color(if is_breached {
                                    theme.danger
                                } else {
                                    theme.accent
                                })
                                .child(if is_breached {
                                    "Low Disk Space (Below threshold)"
                                } else {
                                    "Healthy (Above threshold)"
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Background Service:"),
                        )
                        .child(
                            div()
                                .text_color(service_color)
                                .child(service_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Last Checked:"),
                        )
                        .child(
                            div()
                                .debug_selector(|| {
                                    "watchdog-last-checked".into()
                                })
                                .text_color(theme.bright)
                                .child(format_checked_age(
                                    app.watchdog_space_checked_at,
                                )),
                        ),
                ),
        );

    let actions_card = dialog_popup(cx)
        .w_full()
        .child(dialog_title("Actions", cx))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(space::MD)
                .child(
                    div()
                        .debug_selector(|| "watchdog-check-space".into())
                        .child(
                            button(
                                "watchdog-check-space",
                                "Check Disk Space Now",
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.refresh_watchdog_space(cx);
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "watchdog-ai-analyze".into())
                        .child(
                            button(
                                "watchdog-ai-analyze",
                                if app.watchdog_analyzing {
                                    "Analyzing…"
                                } else {
                                    "Run AI Analysis Now"
                                },
                                if app.watchdog_analyzing {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Primary
                                },
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.run_ai_analysis_now(cx);
                                },
                            )),
                        ),
                ),
        );

    let proposal_card =
        if let Some(ref prop) = app.watchdog_state.last_proposal {
            let total_candidate_bytes: u64 =
                prop.items.iter().map(|i| i.bytes).sum();
            dialog_popup(cx)
                .w_full()
                .child(dialog_title("Latest AI Cleanup Proposal", cx))
                .child(dialog_description(&prop.summary, cx))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .py(space::SM)
                        .child(div().text_color(theme.bright).child(format!(
                            "{} candidate items · {}",
                            prop.items.len(),
                            human_bytes(total_candidate_bytes)
                        )))
                        .child(
                            div()
                                .debug_selector(|| {
                                    "watchdog-review-proposal".into()
                                })
                                .child(
                                    button(
                                        "watchdog-review-proposal",
                                        "Review & Confirm…",
                                        ButtonVariant::Primary,
                                        cx,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_last_proposal(cx);
                                    })),
                                ),
                        ),
                )
        } else {
            dialog_popup(cx)
                .w_full()
                .child(dialog_title("Latest AI Cleanup Proposal", cx))
                .child(dialog_description(
                    "No proposals recorded yet. Run AI Analysis or wait \
                 for watchdog trigger.",
                    cx,
                ))
        };

    div()
        .id("watchdog-page")
        .debug_selector(|| "watchdog-page".into())
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .p(space::XXL)
        .gap(space::LG)
        .overflow_y_scroll()
        .child(div().text_size(text::TITLE).child("Disk Space Watchdog"))
        .child(div().text_color(theme.secondary).child(
            "Low-overhead disk monitoring with proactive AI cleanup \
             suggestions.",
        ))
        .child(status_card)
        .child(actions_card)
        .child(proposal_card)
}

/// Settings configuration page.
pub fn settings_page(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();

    let input_min_free = app
        .watchdog_input_min_free
        .as_ref()
        .expect("inputs initialized");
    let input_custom_cmd = app
        .watchdog_input_custom_cmd
        .as_ref()
        .expect("inputs initialized");
    let input_extra_args = app
        .watchdog_input_extra_args
        .as_ref()
        .expect("inputs initialized");
    let input_model = app
        .watchdog_input_model
        .as_ref()
        .expect("inputs initialized");

    let is_enabled = app.watchdog_config.enabled;
    let is_service = app.watchdog_config.background_service;
    let current_provider = app.watchdog_config.ai_provider;

    let entity_watchdog = cx.entity().downgrade();
    let entity_service = entity_watchdog.clone();

    let watchdog_section = dialog_popup(cx)
        .w_full()
        .child(dialog_title("Watchdog Monitoring", cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .child(
                    div()
                        .debug_selector(|| "setting-enable-watchdog".into())
                        .child(
                            checkbox(
                                "setting-enable-watchdog",
                                "Enable Watchdog Free-Space Monitoring",
                                if is_enabled {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_watchdog.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_config.enabled =
                                                !this.watchdog_config.enabled;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-enable-service".into())
                        .child(
                            checkbox(
                                "setting-enable-service",
                                "Run as OS Background Service at Login \
                                 (launchd / systemd)",
                                if is_service {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_service.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_config
                                                .background_service = !this
                                                .watchdog_config
                                                .background_service;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(
                            div()
                                .text_color(theme.secondary)
                                .child("Minimum Free Space Threshold (GB):"),
                        )
                        .child(
                            gpui_omarchy::input(
                                "setting-min-free-input",
                                input_min_free,
                                window,
                                cx,
                            )
                            .debug_selector(|| "setting-min-free-input".into()),
                        ),
                ),
        );

    let mut provider_buttons = div().flex().flex_row().gap(space::SM);
    for provider in AiCliProvider::ALL {
        let is_selected = provider == current_provider;
        let id = match provider {
            AiCliProvider::Agy => "provider-agy",
            AiCliProvider::Pi => "provider-pi",
            AiCliProvider::Codex => "provider-codex",
            AiCliProvider::Custom => "provider-custom",
        };
        provider_buttons = provider_buttons.child(
            button(
                id,
                provider.label(),
                if is_selected {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                },
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_watchdog_provider(provider, cx);
            })),
        );
    }

    let mut ai_section = dialog_popup(cx)
        .w_full()
        .child(dialog_title("AI Assistant CLI", cx))
        .child(dialog_description(
            "Select which AI CLI to invoke when free space drops \
             below threshold.",
            cx,
        ))
        .child(provider_buttons);

    if current_provider == AiCliProvider::Custom {
        ai_section = ai_section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child("Custom Executable Command / Path:"),
                    )
                    .child(
                        gpui_omarchy::input(
                            "setting-custom-cmd-input",
                            input_custom_cmd,
                            window,
                            cx,
                        )
                        .debug_selector(|| "setting-custom-cmd-input".into()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child("Custom Extra Arguments:"),
                    )
                    .child(
                        gpui_omarchy::input(
                            "setting-extra-args-input",
                            input_extra_args,
                            window,
                            cx,
                        )
                        .debug_selector(|| "setting-extra-args-input".into()),
                    ),
            );
    }

    ai_section = ai_section
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(
                    div()
                        .text_color(theme.secondary)
                        .child("Model Override (optional):"),
                )
                .child(
                    gpui_omarchy::input(
                        "setting-model-input",
                        input_model,
                        window,
                        cx,
                    )
                    .debug_selector(|| "setting-model-input".into()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(space::MD)
                .child(
                    div().debug_selector(|| "setting-test-cli".into()).child(
                        button(
                            "setting-test-cli",
                            if app.watchdog_testing {
                                "Testing…"
                            } else {
                                "Test CLI Connection"
                            },
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.test_watchdog_ai_cli(cx);
                            },
                        )),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-fetch-models".into())
                        .child(
                            button(
                                "setting-fetch-models",
                                if app.watchdog_fetching_models {
                                    "Fetching…"
                                } else {
                                    "Fetch Models"
                                },
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.fetch_watchdog_models(cx);
                                },
                            )),
                        ),
                ),
        );

    if !app.watchdog_models.is_empty() {
        let mut model_rows = div()
            .id("setting-model-list")
            .debug_selector(|| "setting-model-list".into())
            .flex()
            .flex_col()
            .gap(space::XS)
            .max_h(Rems(14.0))
            .overflow_y_scroll();

        let selected_model = app.watchdog_config.model.as_deref();
        for (index, model) in app.watchdog_models.iter().enumerate() {
            let is_selected = selected_model == Some(model.id.as_str());
            let is_benchmarking =
                app.watchdog_benchmarking_models.contains(&model.id);
            let select_id = model.id.clone();
            let select_btn_id = model.id.clone();
            let bench_id = model.id.clone();
            let speed_opt = app
                .watchdog_speed_results
                .iter()
                .find(|r| r.model == model.id);

            let label_suffix =
                (model.label != model.id).then(|| format!("  {}", model.label));

            let speed_tooltip = if is_benchmarking {
                "Benchmarking model speed…"
            } else if speed_opt.is_some() {
                "Click to re-test model speed"
            } else {
                "Test model speed"
            };
            let speed_control = with_tooltip(
                div()
                    .id(ElementId::Name(
                        format!("setting-model-speed-{index}").into(),
                    ))
                    .debug_selector(move || {
                        format!("setting-model-speed-{index}")
                    })
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .px(space::SM)
                    .py(space::XS)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.control_border())
                    .bg(theme.normal_fill())
                    .hover(|style| style.bg(theme.hover_fill()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.benchmark_watchdog_model(
                            Some(bench_id.clone()),
                            cx,
                        );
                    }))
                    .child(if is_benchmarking {
                        spinning_loader(
                            ElementId::Name(
                                format!("setting-model-spin-{index}").into(),
                            ),
                            theme.accent,
                        )
                        .into_any_element()
                    } else if let Some(res) = speed_opt {
                        div()
                            .text_size(text::CAPTION)
                            .text_color(if res.ok {
                                theme.success
                            } else {
                                theme.danger
                            })
                            .child(if res.ok {
                                format!("{} ms", res.elapsed_ms)
                            } else {
                                format!("retry ({} ms)", res.elapsed_ms)
                            })
                            .into_any_element()
                    } else {
                        zap_icon(theme.accent).into_any_element()
                    }),
                speed_tooltip,
            );

            let select_control = if is_selected {
                div()
                    .id(ElementId::Name(
                        format!("setting-model-select-{index}").into(),
                    ))
                    .debug_selector(move || {
                        format!("setting-model-select-{index}")
                    })
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::XXS)
                    .px(space::SM)
                    .py(space::XS)
                    .rounded(space::XS)
                    .bg(theme.accent.opacity(0.18))
                    .text_size(text::CAPTION)
                    .text_color(theme.accent)
                    .child(
                        gpui_omarchy::icon(IconName::CircleCheck)
                            .size(icon::SM)
                            .text_color(theme.accent),
                    )
                    .child("Selected")
                    .into_any_element()
            } else {
                div()
                    .debug_selector(move || {
                        format!("setting-model-select-{index}")
                    })
                    .child(
                        button(
                            ElementId::Name(
                                format!("setting-model-select-btn-{index}")
                                    .into(),
                            ),
                            "Select",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.select_watchdog_model(
                                    &select_btn_id,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .into_any_element()
            };

            model_rows = model_rows.child(
                div()
                    .id(ElementId::Name(
                        format!("setting-model-row-{index}").into(),
                    ))
                    .debug_selector(move || {
                        format!("setting-model-row-{index}")
                    })
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .p(space::SM)
                    .rounded(space::XS)
                    .bg(if is_selected {
                        theme.accent.opacity(0.12)
                    } else {
                        theme.accent.opacity(0.0)
                    })
                    .border_1()
                    .border_color(if is_selected {
                        theme.accent.opacity(0.45)
                    } else {
                        theme.divider()
                    })
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("setting-model-pick-{index}").into(),
                            ))
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .gap(space::SM)
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.select_watchdog_model(
                                        &select_id, window, cx,
                                    );
                                },
                            ))
                            .child(
                                div()
                                    .text_color(if is_selected {
                                        theme.accent
                                    } else {
                                        theme.bright
                                    })
                                    .child(model.id.clone()),
                            )
                            .children(label_suffix.map(|s| {
                                div()
                                    .text_size(text::CAPTION)
                                    .text_color(theme.secondary)
                                    .child(s)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(space::SM)
                            .child(speed_control)
                            .child(select_control),
                    ),
            );
        }

        let any_benchmarking = !app.watchdog_benchmarking_models.is_empty();
        let models_header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(text::CAPTION)
                    .text_color(theme.secondary)
                    .child(format!(
                        "Available Models ({}):",
                        app.watchdog_models.len()
                    )),
            )
            .child(with_tooltip(
                div()
                    .id("setting-speed-all-btn")
                    .debug_selector(|| "setting-speed-all-btn".into())
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(space::XS)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.control_border())
                    .bg(theme.normal_fill())
                    .hover(|style| style.bg(theme.hover_fill()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.benchmark_all_watchdog_models(cx);
                    }))
                    .child(if any_benchmarking {
                        spinning_loader("setting-speed-all-spin", theme.accent)
                            .into_any_element()
                    } else {
                        zap_icon(theme.accent).into_any_element()
                    }),
                "Test speed of all models",
            ));

        ai_section = ai_section.child(models_header).child(model_rows);
    }

    let save_bar = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(space::MD)
        .child(
            div().debug_selector(|| "setting-save-btn".into()).child(
                button(
                    "setting-save-btn",
                    if app.watchdog_saving {
                        "Saving…"
                    } else {
                        "Save Settings"
                    },
                    ButtonVariant::Primary,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.save_watchdog_settings(cx);
                })),
            ),
        );

    div()
        .id("settings-page")
        .debug_selector(|| "settings-page".into())
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .p(space::XXL)
        .gap(space::LG)
        .overflow_y_scroll()
        .child(div().text_size(text::TITLE).child("Settings"))
        .child(div().text_color(theme.secondary).child(
            "Configure Watchdog threshold, background daemon service, \
             and AI assistant.",
        ))
        .child(watchdog_section)
        .child(ai_section)
        .child(save_bar)
}

/// Interactive confirmation dialog for cleanup proposals.
pub fn cleanup_confirm_dialog(
    app: &Ruum,
    modal: &CleanupConfirmModal,
    window: &Window,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let home = std::env::home_dir();

    let rail_px = size::ICON_RAIL.0 * app.rem;
    let available_w =
        (window.viewport_size().width.as_f32() - rail_px).max(0.0);
    let available_h = window.viewport_size().height.as_f32();

    let mut rows = div()
        .id("confirm-item-rows")
        .debug_selector(|| "confirm-item-rows".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .max_h(Rems(16.0))
        .overflow_y_scroll();

    if modal.request.items.is_empty() {
        rows = rows.child(dialog_description(
            "No candidate items proposed for cleanup.",
            cx,
        ));
    }

    for (index, item) in modal.request.items.iter().enumerate() {
        let is_checked = modal.is_checked(&item.path);
        let check_state = if is_checked {
            CheckboxState::Checked
        } else {
            CheckboxState::Unchecked
        };
        let entity_item = cx.entity().downgrade();
        let path_clone = item.path.clone();
        let display_name =
            crate::marks::display_path(&item.path, home.as_deref());

        let mode_label = match item.mode {
            RemovalMode::Trash => "Trash",
            RemovalMode::Permanent => "Permanent",
        };
        let mode_color = match item.mode {
            RemovalMode::Trash => theme.accent,
            RemovalMode::Permanent => theme.danger,
        };

        let mut row = div()
            .id(ElementId::Name(format!("confirm-row-{index}").into()))
            .debug_selector(move || format!("confirm-row-{index}"))
            .flex()
            .flex_col()
            .p(space::SM)
            .rounded(space::XS)
            .bg(if is_checked {
                theme.accent.opacity(0.06)
            } else {
                theme.accent.opacity(0.0)
            })
            .border_1()
            .border_color(if is_checked {
                theme.accent.opacity(0.2)
            } else {
                theme.divider()
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .child(
                        checkbox(
                            ElementId::Name(
                                format!("check-item-{index}").into(),
                            ),
                            display_name,
                            check_state,
                            cx,
                        )
                        .tab_stop(false)
                        .on_change(
                            move |_, _, _, cx| {
                                let _ = entity_item.update(cx, |this, cx| {
                                    if let Some(ref mut m) = this.confirm_modal
                                    {
                                        m.toggle_path(&path_clone);
                                    }
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(space::SM)
                            .child(
                                div()
                                    .text_size(text::CAPTION)
                                    .text_color(mode_color)
                                    .child(mode_label),
                            )
                            .child(
                                div()
                                    .text_size(text::BODY)
                                    .text_color(theme.bright)
                                    .child(human_bytes(item.bytes)),
                            ),
                    ),
            );

        if let Some(ref reason) = item.reason {
            row = row.child(
                div()
                    .text_size(text::CAPTION)
                    .text_color(theme.secondary)
                    .pl(space::LG)
                    .child(reason.clone()),
            );
        }

        if let Some(ref warning) = item.git_warning {
            row = row.child(
                div()
                    .text_size(text::CAPTION)
                    .text_color(theme.danger)
                    .pl(space::LG)
                    .child(format!("⚠️ {warning}")),
            );
        }

        rows = rows.child(row);
    }

    let summary_text = format!(
        "Selected {} of {} items · {}",
        modal.selected_count(),
        modal.request.items.len(),
        human_bytes(modal.selected_bytes())
    );

    let card = dialog_popup(cx)
        .w_full()
        .child(dialog_title(modal.request.title.clone(), cx))
        .child(dialog_description(
            modal
                .request
                .summary
                .as_deref()
                .unwrap_or("Review and uncheck any items you wish to keep."),
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .py(space::XS)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(space::XS)
                        .child(
                            button(
                                "select-all-btn",
                                "Select All",
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    if let Some(ref mut m) = this.confirm_modal
                                    {
                                        m.select_all();
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            button(
                                "select-none-btn",
                                "Select None",
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    if let Some(ref mut m) = this.confirm_modal
                                    {
                                        m.select_none();
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.bright)
                        .child(summary_text),
                ),
        )
        .child(rows)
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::SM)
                .child(
                    div().debug_selector(|| "confirm-cancel".into()).child(
                        dialog_button(
                            "confirm-cancel",
                            "Cancel",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cancel_cleanup_modal(cx);
                            },
                        )),
                    ),
                )
                .child(
                    div().debug_selector(|| "confirm-clean".into()).child(
                        dialog_button(
                            "confirm-clean",
                            format!(
                                "Confirm & Clean ({})",
                                modal.selected_count()
                            ),
                            ButtonVariant::Danger,
                            cx,
                        )
                        .disabled(modal.selected_count() == 0)
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirm_cleanup_modal(cx);
                            },
                        )),
                    ),
                ),
        );

    div()
        .id("cleanup-confirm-backdrop")
        .debug_selector(|| "cleanup-confirm-backdrop".into())
        .absolute()
        .inset_0()
        .bg(theme.background.opacity(0.85))
        .flex()
        .items_center()
        .justify_center()
        .p(space::XL)
        .child(
            div()
                .id("cleanup-confirm-popup")
                .debug_selector(|| "cleanup-confirm-popup".into())
                .w(size::SOURCE_PICKER)
                .max_w(Rems(
                    available_w / app.rem * size::SOURCE_VIEWPORT_FRACTION,
                ))
                .max_h(Rems(
                    available_h / app.rem * size::SOURCE_VIEWPORT_FRACTION,
                ))
                .overflow_y_scroll()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(card),
        )
}
