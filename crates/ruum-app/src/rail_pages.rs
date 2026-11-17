//! Left Icon Rail and full-page navigation views.
//!
//! Provides the primary navigation rail and the full-page views:
//! Sources, Watchdog, and Settings.

use std::time::Duration;

use gpui_kit::base::CheckboxState;
use gpui_kit::base::input::InputState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, Context, Div, ElementId, Entity, FocusHandle,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Rems, Stateful, StatefulInteractiveElement as _,
    Styled, Svg, Transformation, Window, div, radians, svg,
};
use gpui_omarchy::{
    ActiveTheme, ButtonVariant, IconName, button, checkbox, dialog_button,
    dialog_description, dialog_popup, dialog_title, with_tooltip,
};
use ruum_core::cli_install::CliInstallStatus;
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
pub fn icon_rail(app: &Ruum, cx: &Context<'_, Ruum>) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
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
                    crate::i18n::pages::rail_explore(locale),
                    is_explore,
                    |this, _, cx| {
                        this.navigate_rail(Screen::Explore, cx);
                    },
                    cx,
                ))
                .child(rail_button(
                    "rail-sources",
                    IconName::HardDrive,
                    crate::i18n::pages::rail_sources(locale),
                    is_sources,
                    |this, _, cx| {
                        this.navigate_rail(Screen::Sources, cx);
                    },
                    cx,
                ))
                .child(rail_button(
                    "rail-watchdog",
                    IconName::Eye,
                    crate::i18n::pages::rail_watchdog(locale),
                    is_watchdog,
                    |this, _, cx| {
                        this.navigate_rail(Screen::Watchdog, cx);
                    },
                    cx,
                )),
        )
        .child(div().flex().flex_col().items_center().child(rail_button(
            "rail-settings",
            IconName::Settings,
            crate::i18n::pages::rail_settings(
                cfg!(target_os = "macos"),
                locale,
            ),
            is_settings,
            |this, _, cx| {
                this.navigate_rail(Screen::Settings, cx);
            },
            cx,
        )))
}

/// Full-page Sources and Drives selection screen.
pub fn sources_page(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let source_ready = app.picker_source_matches(cx);
    let source = app.volumes_source.as_ref().map_or_else(
        || crate::i18n::pages::this_computer(locale).to_owned(),
        |host| crate::i18n::pages::ssh_read_only(host, locale),
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
            crate::i18n::pages::listing_mounts(locale),
            cx,
        ));
    } else if let Some(error) = &app.volumes_error {
        rows = rows.child(div().text_color(theme.danger).child(error.clone()));
    } else if app.volumes.is_empty() {
        rows = rows.child(dialog_description(
            crate::i18n::pages::no_mounts_listed(locale),
            cx,
        ));
    }

    for (index, volume) in app.volumes.iter().enumerate() {
        let highlighted = index == app.volume_highlight;
        let label = format!(
            "{}  ·  {}",
            volume.point.display(),
            volume
                .device
                .as_deref()
                .unwrap_or_else(|| crate::i18n::pages::mounted_volume(locale))
        );
        rows = rows.child(
            div()
                .id(ElementId::Name(format!("volume-{index}").into()))
                .debug_selector(move || format!("volume-{index}"))
                .px(space::MD)
                .py(space::SM)
                .rounded(space::XS)
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

    let source_card = dialog_popup(cx)
        .w_full()
        .child(dialog_title(
            crate::i18n::pages::card_source_title(locale),
            cx,
        ))
        .child(dialog_description(
            crate::i18n::pages::card_source_desc(locale),
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
                        crate::i18n::pages::this_computer(locale),
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
                        crate::i18n::pages::connect_ssh(locale),
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
        .child(div().text_color(theme.bright).child(source));

    let mounts_card =
        dialog_popup(cx)
            .w_full()
            .child(dialog_title(
                crate::i18n::pages::card_mounts_title(locale),
                cx,
            ))
            .child(dialog_description(
                crate::i18n::pages::card_mounts_desc(locale),
                cx,
            ))
            .child(rows)
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
                            crate::i18n::pages::scan_directory(locale),
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
            .children(app.picker_error.as_ref().map(|error| {
                div().text_color(theme.danger).child(error.clone())
            }))
            .child(
                div().flex().flex_row().justify_end().gap(space::SM).child(
                    dialog_button(
                        "source-scan",
                        crate::i18n::pages::scan_selected_root(locale),
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

    let subtitle = if app.awaiting_root {
        crate::i18n::pages::sources_subtitle_awaiting(locale)
    } else {
        crate::i18n::pages::sources_subtitle_ready(locale)
    };

    let mut subtitle_row = div().text_color(theme.secondary).child(subtitle);
    if app.awaiting_root {
        subtitle_row =
            subtitle_row.debug_selector(|| "choose-root-idle".into());
    }

    div()
        .id("sources-page")
        .debug_selector(|| "sources-page".into())
        .track_focus(&app.confirm_focus)
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .p(space::XXL)
        .gap(space::LG)
        .overflow_hidden()
        .child(
            div()
                .text_size(text::TITLE)
                .child(crate::i18n::pages::sources_title(locale)),
        )
        .child(subtitle_row)
        .child(
            div()
                .id("source-picker")
                .debug_selector(|| "source-picker".into())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap(space::LG)
                .overflow_y_scroll()
                .child(source_card)
                .child(mounts_card),
        )
}

/// Watchdog monitor and recommendation page.
/// Format a unix-seconds check timestamp as a concise human-readable age.
fn format_checked_age(
    checked_secs: Option<u64>,
    locale: crate::i18n::Locale,
) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    crate::i18n::pages::format_checked_age(checked_secs, now, locale)
}

/// Watchdog monitor status, manual trigger, and proposal history.
pub fn watchdog_page(
    app: &Ruum,
    _window: &Window,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let home = std::env::home_dir();
    let home_display = home
        .as_ref()
        .map_or_else(|| "/".to_string(), |h| h.display().to_string());

    let space_opt = app.watchdog_home_space.as_ref().or(if cfg!(test) {
        app.space.as_ref()
    } else {
        None
    });
    let thresh_bytes = app
        .watchdog_config
        .min_free_gb
        .saturating_mul(1024 * 1024 * 1024);
    let (free_display, is_breached) = if let Some(space) = space_opt {
        (
            crate::i18n::pages::free_of_total(
                &human_bytes(space.available),
                &human_bytes(space.total),
                locale,
            ),
            Some(space.available < thresh_bytes),
        )
    } else {
        (crate::i18n::pages::unknown(locale).to_string(), None)
    };
    let (status_text, status_color) = match is_breached {
        Some(true) => {
            (crate::i18n::pages::status_low_disk(locale), theme.danger)
        }
        Some(false) => {
            (crate::i18n::pages::status_healthy(locale), theme.accent)
        }
        None => (crate::i18n::pages::unknown(locale), theme.secondary),
    };

    let service_label =
        crate::i18n::pages::service_status(app.watchdog_service_status, locale);
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
        .child(dialog_title(
            crate::i18n::pages::volume_status_title(locale),
            cx,
        ))
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
                            div().text_color(theme.secondary).child(
                                crate::i18n::pages::monitored_root(locale),
                            ),
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
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::free_space_label(locale),
                        ))
                        .child(
                            div()
                                .text_color(if is_breached == Some(true) {
                                    theme.danger
                                } else {
                                    theme.bright
                                })
                                .child(free_display),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::alert_threshold_label(locale),
                        ))
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
                            div().text_color(theme.secondary).child(
                                crate::i18n::pages::status_label(locale),
                            ),
                        )
                        .child(
                            div()
                                .debug_selector(|| {
                                    "watchdog-status-value".into()
                                })
                                .text_color(status_color)
                                .child(status_text),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::background_service_label(
                                locale,
                            ),
                        ))
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
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::last_checked_label(locale),
                        ))
                        .child(
                            div()
                                .debug_selector(|| {
                                    "watchdog-last-checked".into()
                                })
                                .text_color(theme.bright)
                                .child(format_checked_age(
                                    app.watchdog_space_checked_at,
                                    locale,
                                )),
                        ),
                ),
        );

    let actions_card = dialog_popup(cx)
        .w_full()
        .child(dialog_title(crate::i18n::pages::actions_title(locale), cx))
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
                                crate::i18n::pages::check_disk_space_now(
                                    locale,
                                ),
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
                                    crate::i18n::pages::analyzing(locale)
                                } else {
                                    crate::i18n::pages::run_ai_analysis_now(
                                        locale,
                                    )
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
                .child(dialog_title(
                    crate::i18n::pages::latest_proposal_title(locale),
                    cx,
                ))
                .child(dialog_description(&prop.summary, cx))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .py(space::SM)
                        .child(div().text_color(theme.bright).child(
                            crate::i18n::pages::candidate_items_summary(
                                prop.items.len(),
                                &human_bytes(total_candidate_bytes),
                                locale,
                            ),
                        ))
                        .child(
                            div()
                                .debug_selector(|| {
                                    "watchdog-review-proposal".into()
                                })
                                .child(
                                    button(
                                        "watchdog-review-proposal",
                                        crate::i18n::pages::review_and_confirm(
                                            locale,
                                        ),
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
                .child(dialog_title(
                    crate::i18n::pages::latest_proposal_title(locale),
                    cx,
                ))
                .child(dialog_description(
                    crate::i18n::pages::no_proposals_yet(locale),
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
        .child(
            div()
                .text_size(text::TITLE)
                .child(crate::i18n::pages::watchdog_title(locale)),
        )
        .child(
            div()
                .text_color(theme.secondary)
                .child(crate::i18n::pages::watchdog_subtitle(locale)),
        )
        .child(status_card)
        .child(actions_card)
        .child(proposal_card)
}

fn setting_input_field(
    id: &'static str,
    label: &'static str,
    state: &Entity<InputState>,
    label_color: Hsla,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(div().text_color(label_color).child(label))
        .child(
            gpui_omarchy::input(id, state, window, cx)
                .debug_selector(move || id.into()),
        )
}

/// Settings configuration page.
pub fn settings_page(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;

    let input_min_free = app
        .watchdog_input_min_free
        .as_ref()
        .expect("inputs initialized");
    let input_poll_interval = app
        .watchdog_input_poll_interval
        .as_ref()
        .expect("inputs initialized");
    let input_cooldown = app
        .watchdog_input_cooldown
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

    let is_enabled = app.watchdog_draft.enabled;
    let is_service = app.watchdog_draft.background_service;
    let is_auto_analyze = app.watchdog_draft.ai_auto_analyze;
    let current_provider = app.watchdog_draft.ai_provider;

    let entity_watchdog = cx.entity().downgrade();
    let entity_service = entity_watchdog.clone();
    let entity_auto_analyze = entity_watchdog.clone();

    let watchdog_section = dialog_popup(cx)
        .w_full()
        .child(dialog_title(
            crate::i18n::pages::watchdog_monitoring_title(locale),
            cx,
        ))
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
                                crate::i18n::pages::enable_watchdog(locale),
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
                                            this.watchdog_draft.enabled =
                                                !this.watchdog_draft.enabled;
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
                                crate::i18n::pages::enable_service(locale),
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
                                            this.watchdog_draft
                                                .background_service = !this
                                                .watchdog_draft
                                                .background_service;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(setting_input_field(
                    "setting-min-free-input",
                    crate::i18n::pages::min_free_threshold(locale),
                    input_min_free,
                    theme.secondary,
                    window,
                    cx,
                ))
                .child(setting_input_field(
                    "setting-poll-interval-input",
                    crate::i18n::pages::poll_interval_minutes(locale),
                    input_poll_interval,
                    theme.secondary,
                    window,
                    cx,
                ))
                .child(setting_input_field(
                    "setting-cooldown-input",
                    crate::i18n::pages::cooldown_minutes(locale),
                    input_cooldown,
                    theme.secondary,
                    window,
                    cx,
                )),
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
                provider.label_in(locale),
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
        .child(dialog_title(
            crate::i18n::pages::ai_assistant_cli_title(locale),
            cx,
        ))
        .child(dialog_description(
            crate::i18n::pages::ai_assistant_cli_desc(locale),
            cx,
        ))
        .child(
            div()
                .debug_selector(|| "setting-ai-auto-analyze".into())
                .child(
                    checkbox(
                        "setting-ai-auto-analyze",
                        crate::i18n::pages::ai_auto_analyze(locale),
                        if is_auto_analyze {
                            CheckboxState::Checked
                        } else {
                            CheckboxState::Unchecked
                        },
                        cx,
                    )
                    .tab_stop(false)
                    .on_change(move |_, _, _, cx| {
                        let _ = entity_auto_analyze.update(cx, |this, cx| {
                            this.watchdog_draft.ai_auto_analyze =
                                !this.watchdog_draft.ai_auto_analyze;
                            cx.notify();
                        });
                    }),
                ),
        )
        .child(provider_buttons);

    let show_custom_cmd = current_provider == AiCliProvider::Custom
        || !app.watchdog_config.custom_command.is_empty()
        || !input_custom_cmd.read(cx).value().trim().is_empty();
    if show_custom_cmd {
        ai_section = ai_section.child(setting_input_field(
            "setting-custom-cmd-input",
            crate::i18n::pages::custom_executable(locale),
            input_custom_cmd,
            theme.secondary,
            window,
            cx,
        ));
    }

    ai_section = ai_section
        .child(setting_input_field(
            "setting-extra-args-input",
            crate::i18n::pages::custom_extra_args(locale),
            input_extra_args,
            theme.secondary,
            window,
            cx,
        ))
        .child(setting_input_field(
            "setting-model-input",
            crate::i18n::pages::model_override(locale),
            input_model,
            theme.secondary,
            window,
            cx,
        ))
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
                                crate::i18n::pages::testing(locale)
                            } else {
                                crate::i18n::pages::test_cli_connection(locale)
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
                                    crate::i18n::pages::fetching(locale)
                                } else {
                                    crate::i18n::pages::fetch_models(locale)
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
            .overflow_y_scroll()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        let selected_model = app.watchdog_draft.model.as_deref();
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
                crate::i18n::pages::benchmarking_speed_tooltip(locale)
            } else if speed_opt.is_some() {
                crate::i18n::pages::retest_speed_tooltip(locale)
            } else {
                crate::i18n::pages::test_speed_tooltip(locale)
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
                                crate::i18n::pages::speed_retry(
                                    u128::from(res.elapsed_ms),
                                    locale,
                                )
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
                    .child(crate::i18n::pages::selected(locale))
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
                            crate::i18n::pages::select(locale),
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
                    .child(crate::i18n::pages::available_models(
                        app.watchdog_models.len(),
                        locale,
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
                crate::i18n::pages::test_all_models_tooltip(locale),
            ));

        ai_section = ai_section.child(models_header).child(model_rows);
    }

    let (cli_status_text, cli_in_path_hint, cli_installed) =
        match &app.cli_install_status {
            Some(CliInstallStatus::Installed { path, in_path }) => {
                let path_str = path.display().to_string();
                let hint = (!in_path).then(|| {
                    let dir = path
                        .parent()
                        .map_or_else(String::new, |d| d.display().to_string());
                    crate::i18n::pages::cli_not_in_path_hint(&dir, locale)
                });
                (
                    crate::i18n::pages::cli_status_installed(&path_str, locale),
                    hint,
                    true,
                )
            }
            Some(CliInstallStatus::NotInstalled) | None => (
                crate::i18n::pages::cli_status_not_installed(locale).to_owned(),
                None,
                false,
            ),
        };
    let cli_btn_label = if app.cli_updating {
        crate::i18n::pages::cli_updating_btn(locale)
    } else if cli_installed {
        crate::i18n::pages::cli_uninstall_btn(locale)
    } else {
        crate::i18n::pages::cli_install_btn(locale)
    };
    let cli_section = dialog_popup(cx)
        .w_full()
        .child(dialog_title(crate::i18n::pages::cli_tool_title(locale), cx))
        .child(dialog_description(
            crate::i18n::pages::cli_tool_desc(locale),
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::MD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XXS)
                        .child(
                            div()
                                .text_color(theme.bright)
                                .child(cli_status_text),
                        )
                        .children(cli_in_path_hint.map(|hint| {
                            div()
                                .text_size(text::CAPTION)
                                .text_color(theme.danger)
                                .child(hint)
                        })),
                )
                .child(
                    div().debug_selector(|| "setting-cli-btn".into()).child(
                        button(
                            "setting-cli-btn",
                            cli_btn_label,
                            if cli_installed || app.cli_updating {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Primary
                            },
                            cx,
                        )
                        .disabled(app.cli_updating)
                        .on_click(cx.listener(
                            move |this, _, _, cx| {
                                if cli_installed {
                                    this.uninstall_ruum_cli(cx);
                                } else {
                                    this.install_or_update_cli(cx);
                                }
                            },
                        )),
                    ),
                ),
        );

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
                        crate::i18n::pages::saving(locale)
                    } else {
                        crate::i18n::pages::save_settings(locale)
                    },
                    ButtonVariant::Primary,
                    cx,
                )
                .on_click(cx.listener(
                    |this, _, window, cx| {
                        this.save_watchdog_settings(window, cx);
                    },
                )),
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
        .child(
            div()
                .text_size(text::TITLE)
                .child(crate::i18n::pages::settings_title(locale)),
        )
        .child(
            div()
                .text_color(theme.secondary)
                .child(crate::i18n::pages::settings_subtitle(locale)),
        )
        .child(watchdog_section)
        .child(ai_section)
        .child(cli_section)
        .child(save_bar)
}

/// Standalone interactive confirmation window content for cleanup proposals.
pub fn cleanup_confirm_view(
    modal: &CleanupConfirmModal,
    focus: &FocusHandle,
    locale: crate::i18n::Locale,
    cx: &Context<'_, crate::CleanupConfirmWindow>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let home = std::env::home_dir();

    let mut rows = div()
        .id("confirm-item-rows")
        .debug_selector(|| "confirm-item-rows".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

    if modal.request.items.is_empty() {
        rows = rows.child(dialog_description(
            crate::i18n::pages::no_candidate_items(locale),
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
            RemovalMode::Trash => crate::i18n::pages::mode_trash_short(locale),
            RemovalMode::Permanent => {
                crate::i18n::pages::mode_permanent_short(locale)
            }
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
                        div()
                            .debug_selector(move || {
                                format!("check-item-{index}")
                            })
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
                                        let _ = entity_item.update(
                                            cx,
                                            |this, cx| {
                                                this.modal
                                                    .toggle_path(&path_clone);
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

    let summary_text = crate::i18n::pages::confirm_summary(
        modal.selected_count(),
        modal.request.items.len(),
        &human_bytes(modal.selected_bytes()),
        locale,
    );

    let card = div()
        .id("cleanup-confirm-popup")
        .debug_selector(|| "cleanup-confirm-popup".into())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            dialog_popup(cx)
                .w_full()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(dialog_title(modal.request.title.clone(), cx))
                .child(dialog_description(
                    modal.request.summary.as_deref().unwrap_or_else(|| {
                        crate::i18n::pages::confirm_default_desc(locale)
                    }),
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
                                        crate::i18n::pages::select_all(locale),
                                        ButtonVariant::Secondary,
                                        cx,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.modal.select_all();
                                        cx.notify();
                                    })),
                                )
                                .child(
                                    button(
                                        "select-none-btn",
                                        crate::i18n::pages::select_none(locale),
                                        ButtonVariant::Secondary,
                                        cx,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.modal.select_none();
                                        cx.notify();
                                    })),
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
                            div()
                                .debug_selector(|| "confirm-cancel".into())
                                .child(
                                    dialog_button(
                                        "confirm-cancel",
                                        crate::i18n::pages::cancel(locale),
                                        ButtonVariant::Secondary,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.cancel(window, cx);
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| "confirm-clean".into())
                                .child(
                                    dialog_button(
                                        "confirm-clean",
                                        crate::i18n::pages::confirm_and_clean(
                                            modal.selected_count(),
                                            locale,
                                        ),
                                        ButtonVariant::Danger,
                                        cx,
                                    )
                                    .disabled(modal.selected_count() == 0)
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.confirm(window, cx);
                                        },
                                    )),
                                ),
                        ),
                ),
        );

    div()
        .id("cleanup-confirm-window")
        .debug_selector(|| "cleanup-confirm-window".into())
        .track_focus(focus)
        .key_context("CleanupConfirm")
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => this.cancel(window, cx),
                "enter" => this.confirm(window, cx),
                _ => {}
            }
        }))
        .flex()
        .flex_col()
        .size_full()
        .p(space::XL)
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(theme.font)
        .text_size(text::BODY)
        .child(card)
}
