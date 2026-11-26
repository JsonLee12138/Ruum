//! Left Icon Rail and full-page navigation views.
//!
//! Provides the primary navigation rail and the full-page views:
//! Docker, Remote, Watchdog, and Settings.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::CheckboxState;
use gpui_kit::base::input::InputState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, Div, ElementId, Entity,
    FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Rems, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Svg, Transformation, Window, div,
    radians, relative, svg,
};
use gpui_omarchy::{
    ActiveTheme, ButtonVariant, ChoiceItem, IconName, button, button_group,
    checkbox, dialog_button, dialog_description, dialog_popup, dialog_title,
    with_tooltip,
};
use ruum_core::cli_install::CliInstallStatus;
use ruum_core::docker_adapter::{
    DockerCleanTarget, DockerProbeStatus, DockerUsageSummary,
};
use ruum_core::removal::RemovalMode;
use ruum_core::size::human_bytes;
use ruum_core::storage::{AuthMode, UpdateChannel};
use ruum_core::updater::{Manifest, UpdateCheckResult};
use ruum_core::watchdog::ServiceStatus;
use ruum_core::watchdog_config::{AiCliProvider, ApiProtocol};

use crate::palette;
use crate::power::PowerEfficiency;
use crate::state::{
    CleanupConfirmModal, DockerGroupKind, RemoteConnectConfirmModal, Ruum,
    Screen, SettingsCategory, UpdateUiState, format_unix_date_ymd,
};
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

const PENCIL_SVG: &[u8] = br#"<svg
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
  <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
  <path d="m15 5 4 4" />
</svg>"#;

const TRASH_SVG: &[u8] = br#"<svg
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
  <path d="M3 6h18" />
  <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
  <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
</svg>"#;

const CIRCLE_SVG: &[u8] = br#"<svg
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
  <circle cx="12" cy="12" r="10" />
</svg>"#;

fn zap_icon(color: Hsla) -> Svg {
    svg()
        .data(ZAP_SVG)
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
}

fn pencil_icon(color: Hsla) -> Svg {
    svg()
        .data(PENCIL_SVG)
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
}

fn trash_icon(color: Hsla) -> Svg {
    svg()
        .data(TRASH_SVG)
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
}

fn circle_icon(color: Hsla) -> Svg {
    svg()
        .data(CIRCLE_SVG)
        .size(icon::MD)
        .flex_shrink_0()
        .text_color(color)
}

pub fn action_button_with_spinner(
    id: impl Into<ElementId>,
    spin_id: impl Into<SharedString>,
    label: &'static str,
    loading: bool,
    variant: ButtonVariant,
    cx: &gpui_kit::App,
) -> gpui_omarchy::Button {
    let theme = cx.omarchy();
    let spin_color = if variant == ButtonVariant::Primary {
        theme.accent
    } else {
        theme.foreground
    };
    button(id, "", variant, cx)
        .accessibility_label(label)
        .when(loading, |btn| {
            btn.child(spinning_loader(spin_id, spin_color))
        })
        .child(label)
}

pub fn spinning_loader(
    id: impl Into<SharedString>,
    color: Hsla,
) -> impl IntoElement {
    let sel: SharedString = id.into();
    let debug_sel = sel.to_string();
    div()
        .debug_selector(move || debug_sel)
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(IconName::LoaderCircle.path())
                .size(icon::MD)
                .flex_shrink_0()
                .text_color(color)
                .with_animation(
                    ElementId::Name(sel),
                    Animation::new(Duration::from_millis(900)).repeat(),
                    |this, phase| {
                        this.with_transformation(Transformation::rotate(
                            radians(phase * std::f32::consts::TAU),
                        ))
                    },
                ),
        )
}

/// Render a single navigation icon button in the Left Icon Rail.
fn rail_button(
    id: &'static str,
    icon_name: IconName,
    tooltip_text: impl Into<SharedString>,
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

fn rail_update_button(
    tooltip_text: SharedString,
    active: bool,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let bg_color = if active {
        theme.accent.opacity(0.18)
    } else {
        theme.accent.opacity(0.0)
    };
    let fg_color = if active { theme.bright } else { theme.accent };
    let border_color = if active {
        theme.accent.opacity(0.4)
    } else {
        theme.accent.opacity(0.0)
    };

    with_tooltip(
        div()
            .id("rail-update")
            .debug_selector(|| "rail-update".into())
            .relative()
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
            .child(gpui_omarchy::icon(IconName::Download))
            .child(
                div()
                    .id("rail-update-dot")
                    .debug_selector(|| "rail-update-dot".into())
                    .absolute()
                    .top(space::XS)
                    .right(space::XS)
                    .size(size::UPDATE_DOT)
                    .rounded_full()
                    .bg(theme.accent),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_update_modal(cx);
            })),
        tooltip_text,
    )
}

/// Global left navigation rail.
pub fn icon_rail(app: &Ruum, cx: &Context<'_, Ruum>) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let is_explore = app.screen == Screen::Explore;
    let is_docker = app.screen == Screen::Docker;
    let is_remote = app.screen == Screen::Remote;
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
                .when(app.has_local_docker_daemon(), |rail| {
                    rail.child(rail_button(
                        "rail-docker",
                        IconName::Container,
                        crate::i18n::pages::rail_docker(locale),
                        is_docker,
                        |this, _, cx| {
                            this.navigate_rail(Screen::Docker, cx);
                        },
                        cx,
                    ))
                })
                .child(rail_button(
                    "rail-remote",
                    IconName::Network,
                    crate::i18n::pages::rail_remote(locale),
                    is_remote,
                    |this, _, cx| {
                        this.navigate_rail(Screen::Remote, cx);
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
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(space::SM)
                .when(
                    app.update_state
                        .should_show_rail_entry(&app.update_settings),
                    |col| {
                        let tooltip: SharedString = match &app.update_state {
                            UpdateUiState::Downloading { .. } => {
                                crate::i18n::pages::rail_update_downloading(
                                    locale,
                                )
                                .into()
                            }
                            UpdateUiState::ReadyToInstall { info, .. } => {
                                crate::i18n::pages::rail_update_ready(
                                    &info.latest_version,
                                    locale,
                                )
                                .into()
                            }
                            UpdateUiState::Available { info }
                            | UpdateUiState::Installing { info } => {
                                crate::i18n::pages::rail_update_available(
                                    &info.latest_version,
                                    locale,
                                )
                                .into()
                            }
                            _ => crate::i18n::pages::rail_update(locale).into(),
                        };
                        col.child(rail_update_button(
                            tooltip,
                            app.update_modal_open,
                            cx,
                        ))
                    },
                )
                .child(rail_button(
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
                )),
        )
}

fn ssh_profiles_card(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;

    let mut card = dialog_popup(cx).w_full().child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(space::MD)
            .child(dialog_title(
                crate::i18n::pages::ssh_servers_title(locale),
                cx,
            ))
            .child(
                div().debug_selector(|| "ssh-add-btn".into()).child(
                    with_tooltip(
                        gpui_omarchy::icon_button(
                            "ssh-add-btn",
                            IconName::Plus,
                            crate::i18n::pages::add_ssh_server(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.open_add_ssh_form(cx);
                            },
                        )),
                        crate::i18n::pages::add_ssh_server(locale),
                    ),
                ),
            ),
    );

    if app.ssh_profiles.is_empty() && !app.ssh_form_open {
        card = card.child(dialog_description(
            crate::i18n::pages::no_ssh_servers(locale),
            cx,
        ));
    } else if !app.ssh_profiles.is_empty() {
        let mut list = div().flex().flex_col().gap(space::SM);
        for (index, profile) in app.ssh_profiles.iter().enumerate() {
            let auth_badge = match profile.auth_mode {
                AuthMode::PrivateKey => {
                    crate::i18n::pages::auth_badge_private_key(locale)
                }
                AuthMode::Password => {
                    crate::i18n::pages::auth_badge_password(locale)
                }
            };
            let endpoint = format!(
                "{}@{}:{}",
                profile.username, profile.host, profile.port
            );
            let space_line = match (
                profile.last_available_bytes,
                profile.last_total_bytes,
            ) {
                (Some(avail), Some(total)) => {
                    let age =
                        format_checked_age(profile.last_scanned_secs, locale);
                    crate::i18n::pages::ssh_cached_space(
                        &human_bytes(avail),
                        &human_bytes(total),
                        &age,
                        locale,
                    )
                }
                _ => crate::i18n::pages::ssh_never_scanned(locale).to_owned(),
            };
            let is_scanning = app.is_ssh_profile_scanning(&profile.id);
            let is_checking = app.ssh_checking_profiles.contains(&profile.id);
            let is_analyzing = app.ssh_analyzing_profiles.contains(&profile.id);
            let ssh_ai_stage = if is_analyzing {
                crate::ai_stage_for_key(
                    &crate::AiEventKey::DiskCleanupSsh(profile.id.clone()),
                    cx,
                )
            } else {
                None
            };
            let ssh_ai_tooltip = if is_analyzing {
                crate::i18n::explore::ai_stage_tooltip(ssh_ai_stage, locale)
            } else {
                crate::i18n::pages::ssh_ai_analyze_tooltip(locale)
            };
            let id_connect = profile.id.clone();
            let id_browse = profile.id.clone();
            let id_check = profile.id.clone();
            let id_ai = profile.id.clone();
            let id_edit = profile.id.clone();
            let id_delete = profile.id.clone();

            let connect_btn = div()
                .debug_selector(move || format!("ssh-connect-{index}"))
                .child(with_tooltip(
                    if is_scanning {
                        button(
                            ElementId::Name(
                                format!("ssh-connect-{index}").into(),
                            ),
                            "",
                            ButtonVariant::Primary,
                            cx,
                        )
                        .accessibility_label(
                            crate::i18n::pages::connect_and_scan(locale),
                        )
                        .p(space::SM)
                        .justify_center()
                        .child(spinning_loader(
                            format!("ssh-connect-spin-{index}"),
                            theme.accent,
                        ))
                    } else {
                        gpui_omarchy::icon_button(
                            ElementId::Name(
                                format!("ssh-connect-{index}").into(),
                            ),
                            IconName::Play,
                            crate::i18n::pages::connect_and_scan(locale),
                            ButtonVariant::Primary,
                            cx,
                        )
                    }
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.connect_ssh_profile(&id_connect, cx);
                        },
                    )),
                    crate::i18n::pages::connect_and_scan(locale),
                ));

            let browse_btn = div()
                .debug_selector(move || format!("ssh-browse-{index}"))
                .child(with_tooltip(
                    gpui_omarchy::icon_button(
                        ElementId::Name(format!("ssh-browse-{index}").into()),
                        IconName::FolderOpen,
                        crate::i18n::pages::ssh_browse_dirs_tooltip(locale),
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.open_remote_volumes_modal(&id_browse, cx);
                        },
                    )),
                    crate::i18n::pages::ssh_browse_dirs_tooltip(locale),
                ));

            let check_btn = div()
                .debug_selector(move || format!("ssh-check-{index}"))
                .child(with_tooltip(
                    if is_checking {
                        button(
                            ElementId::Name(
                                format!("ssh-check-{index}").into(),
                            ),
                            "",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .accessibility_label(
                            crate::i18n::pages::ssh_check_space_tooltip(locale),
                        )
                        .p(space::SM)
                        .justify_center()
                        .child(spinning_loader(
                            format!("ssh-check-spin-{index}"),
                            theme.foreground,
                        ))
                    } else {
                        gpui_omarchy::icon_button(
                            ElementId::Name(
                                format!("ssh-check-{index}").into(),
                            ),
                            IconName::HardDrive,
                            crate::i18n::pages::ssh_check_space_tooltip(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                    }
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.check_ssh_profile_space(&id_check, cx);
                        },
                    )),
                    crate::i18n::pages::ssh_check_space_tooltip(locale),
                ));

            let ai_btn = div()
                .debug_selector(move || format!("ssh-ai-{index}"))
                .child(with_tooltip(
                    if is_analyzing {
                        let mut row = div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(space::XS)
                            .child(spinning_loader(
                                format!("ssh-ai-spin-{index}"),
                                theme.foreground,
                            ));
                        if let Some(stage) = ssh_ai_stage {
                            row = row.child(
                                div()
                                    .text_size(text::CAPTION)
                                    .text_color(theme.foreground)
                                    .whitespace_nowrap()
                                    .child(
                                        crate::i18n::explore::ai_stage_compact_label(
                                            stage, locale,
                                        ),
                                    ),
                            );
                        }
                        button(
                            ElementId::Name(format!("ssh-ai-{index}").into()),
                            "",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .accessibility_label(ssh_ai_tooltip)
                        .p(space::SM)
                        .justify_center()
                        .child(row)
                    } else {
                        gpui_omarchy::icon_button(
                            ElementId::Name(format!("ssh-ai-{index}").into()),
                            IconName::Bot,
                            ssh_ai_tooltip,
                            ButtonVariant::Secondary,
                            cx,
                        )
                    }
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.analyze_ssh_profile(&id_ai, cx);
                        },
                    )),
                    ssh_ai_tooltip,
                ));

            let edit_btn = div()
                .debug_selector(move || format!("ssh-edit-{index}"))
                .child(with_tooltip(
                    button(
                        ElementId::Name(format!("ssh-edit-{index}").into()),
                        "",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .accessibility_label(crate::i18n::pages::edit_ssh_server(
                        locale,
                    ))
                    .p(space::SM)
                    .justify_center()
                    .child(pencil_icon(theme.foreground))
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.open_edit_ssh_form(&id_edit, cx);
                        },
                    )),
                    crate::i18n::pages::edit_ssh_server(locale),
                ));

            let delete_btn = div()
                .debug_selector(move || format!("ssh-delete-{index}"))
                .child(with_tooltip(
                    button(
                        ElementId::Name(format!("ssh-delete-{index}").into()),
                        "",
                        ButtonVariant::Danger,
                        cx,
                    )
                    .accessibility_label(crate::i18n::pages::delete_ssh_server(
                        locale,
                    ))
                    .p(space::SM)
                    .justify_center()
                    .child(trash_icon(theme.danger))
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.delete_ssh_profile_card(&id_delete, cx);
                        },
                    )),
                    crate::i18n::pages::delete_ssh_server(locale),
                ));

            let pending_banner = profile.pending_script_path.as_ref().map(|p| {
                let id_run = profile.id.clone();
                let id_discard = profile.id.clone();
                let script_run = PathBuf::from(p);
                let script_view = PathBuf::from(p);
                let banner_text = crate::i18n::pages::ssh_pending_script_banner(
                    profile.pending_script_items.unwrap_or(0),
                    &human_bytes(profile.pending_reclaim_bytes.unwrap_or(0)),
                    locale,
                );
                div()
                    .id(ElementId::Name(
                        format!("ssh-pending-banner-{index}").into(),
                    ))
                    .debug_selector(move || {
                        format!("ssh-pending-banner-{index}")
                    })
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(space::SM)
                    .px(space::SM)
                    .py(space::XS)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.accent.opacity(0.35))
                    .bg(theme.accent.opacity(0.08))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(banner_text),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(space::XS)
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("ssh-pending-run-{index}")
                                    })
                                    .child(
                                        button(
                                            ElementId::Name(
                                                format!(
                                                    "ssh-pending-run-{index}"
                                                )
                                                .into(),
                                            ),
                                            crate::i18n::pages::ssh_pending_run(
                                                locale,
                                            ),
                                            ButtonVariant::Primary,
                                            cx,
                                        )
                                        .on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.execute_cleanup_script_in_terminal(
                                                    &id_run,
                                                    &script_run,
                                                    cx,
                                                );
                                            },
                                        )),
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("ssh-pending-view-{index}")
                                    })
                                    .child(
                                        button(
                                            ElementId::Name(
                                                format!(
                                                    "ssh-pending-view-{index}"
                                                )
                                                .into(),
                                            ),
                                            crate::i18n::pages::ssh_pending_view(
                                                locale,
                                            ),
                                            ButtonVariant::Secondary,
                                            cx,
                                        )
                                        .on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.reveal_cleanup_script(
                                                    &script_view,
                                                    cx,
                                                );
                                            },
                                        )),
                                    ),
                            )
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("ssh-pending-discard-{index}")
                                    })
                                    .child(
                                        button(
                                            ElementId::Name(
                                                format!(
                                                    "ssh-pending-discard-{index}"
                                                )
                                                .into(),
                                            ),
                                            crate::i18n::pages::ssh_pending_discard(
                                                locale,
                                            ),
                                            ButtonVariant::Secondary,
                                            cx,
                                        )
                                        .on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.discard_ssh_pending_script(
                                                    &id_discard,
                                                    cx,
                                                );
                                            },
                                        )),
                                    ),
                            ),
                    )
            });

            list = list.child(
                div()
                    .id(ElementId::Name(format!("ssh-card-{index}").into()))
                    .debug_selector(move || format!("ssh-card-{index}"))
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .p(space::MD)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.divider())
                    .bg(theme.normal_fill())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(space::SM)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(space::SM)
                                    .child(
                                        div()
                                            .text_size(text::BODY)
                                            .text_color(theme.bright)
                                            .child(profile.alias.clone()),
                                    )
                                    .child(
                                        div()
                                            .px(space::XS)
                                            .py(space::XXS)
                                            .rounded(space::XS)
                                            .bg(theme.accent.opacity(0.16))
                                            .text_size(text::CAPTION)
                                            .text_color(theme.accent)
                                            .child(auth_badge),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(space::XS)
                                    .child(connect_btn)
                                    .child(browse_btn)
                                    .child(check_btn)
                                    .child(ai_btn)
                                    .child(edit_btn)
                                    .child(delete_btn),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(space::MD)
                            .text_size(text::CAPTION)
                            .text_color(theme.secondary)
                            .child(endpoint)
                            .children(profile.default_path.as_ref().map(|p| {
                                crate::i18n::pages::ssh_default_path_display(
                                    p, locale,
                                )
                            }))
                            .child(space_line),
                    )
                    .children(pending_banner),
            );
        }
        card = card.child(list);
    }

    if app.ssh_form_open
        && let (
            Some(alias_in),
            Some(host_in),
            Some(port_in),
            Some(user_in),
            Some(path_in),
            Some(pass_in),
            Some(key_in),
            Some(phrase_in),
        ) = (
            app.ssh_input_alias.as_ref(),
            app.ssh_input_host.as_ref(),
            app.ssh_input_port.as_ref(),
            app.ssh_input_username.as_ref(),
            app.ssh_input_default_path.as_ref(),
            app.ssh_input_password.as_ref(),
            app.ssh_input_private_key.as_ref(),
            app.ssh_input_passphrase.as_ref(),
        )
    {
        let form_title = if app.ssh_form_editing_id.is_some() {
            crate::i18n::pages::ssh_form_edit_title(locale)
        } else {
            crate::i18n::pages::ssh_form_add_title(locale)
        };
        let is_key = app.ssh_form_auth_mode == AuthMode::PrivateKey;
        let auth_row = div()
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(
                div()
                    .text_color(theme.secondary)
                    .child(crate::i18n::pages::ssh_form_auth_mode(locale)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(space::SM)
                    .child(
                        div()
                            .debug_selector(|| "ssh-form-auth-key".into())
                            .child(
                                button(
                                    "ssh-form-auth-key",
                                    crate::i18n::pages::auth_badge_private_key(
                                        locale,
                                    ),
                                    if is_key {
                                        ButtonVariant::Primary
                                    } else {
                                        ButtonVariant::Secondary
                                    },
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.set_ssh_form_auth_mode(
                                            AuthMode::PrivateKey,
                                            cx,
                                        );
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "ssh-form-auth-password".into())
                            .child(
                                button(
                                    "ssh-form-auth-password",
                                    crate::i18n::pages::auth_badge_password(
                                        locale,
                                    ),
                                    if is_key {
                                        ButtonVariant::Secondary
                                    } else {
                                        ButtonVariant::Primary
                                    },
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.set_ssh_form_auth_mode(
                                            AuthMode::Password,
                                            cx,
                                        );
                                    }),
                                ),
                            ),
                    ),
            );

        let mut form = div()
            .id("ssh-form")
            .debug_selector(|| "ssh-form".into())
            .flex()
            .flex_col()
            .gap(space::SM)
            .p(space::MD)
            .rounded(space::XS)
            .border_1()
            .border_color(theme.control_border())
            .bg(theme.normal_fill())
            .child(dialog_title(form_title, cx))
            .child(setting_input_field(
                "ssh-form-alias",
                crate::i18n::pages::ssh_form_alias(locale),
                alias_in,
                theme.secondary,
                window,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(space::SM)
                    .child(
                        setting_input_field(
                            "ssh-form-host",
                            crate::i18n::pages::ssh_form_host(locale),
                            host_in,
                            theme.secondary,
                            window,
                            cx,
                        )
                        .flex_1(),
                    )
                    .child(
                        setting_input_field(
                            "ssh-form-port",
                            crate::i18n::pages::ssh_form_port(locale),
                            port_in,
                            theme.secondary,
                            window,
                            cx,
                        )
                        .w(Rems(6.0)),
                    ),
            )
            .child(setting_input_field(
                "ssh-form-username",
                crate::i18n::pages::ssh_form_username(locale),
                user_in,
                theme.secondary,
                window,
                cx,
            ))
            .child(setting_input_field(
                "ssh-form-default-path",
                crate::i18n::pages::ssh_form_default_path(locale),
                path_in,
                theme.secondary,
                window,
                cx,
            ))
            .child(auth_row);

        if is_key {
            form = form
                .child(setting_input_field(
                    "ssh-form-private-key",
                    crate::i18n::pages::ssh_form_private_key(locale),
                    key_in,
                    theme.secondary,
                    window,
                    cx,
                ))
                .child(setting_input_field(
                    "ssh-form-passphrase",
                    crate::i18n::pages::ssh_form_passphrase(locale),
                    phrase_in,
                    theme.secondary,
                    window,
                    cx,
                ));
        } else {
            form = form.child(setting_input_field(
                "ssh-form-password",
                crate::i18n::pages::ssh_form_password(locale),
                pass_in,
                theme.secondary,
                window,
                cx,
            ));
        }

        if app.ssh_form_editing_id.is_some() {
            form = form.child(
                div()
                    .text_size(text::CAPTION)
                    .text_color(theme.secondary)
                    .child(crate::i18n::pages::placeholder_ssh_secret_keep(
                        locale,
                    )),
            );
        }
        if let Some(ref err) = app.ssh_form_error {
            form = form.child(
                div()
                    .debug_selector(|| "ssh-form-error".into())
                    .text_size(text::CAPTION)
                    .text_color(theme.danger)
                    .child(err.clone()),
            );
        }

        form = form.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::SM)
                .child(
                    div().debug_selector(|| "ssh-form-cancel".into()).child(
                        button(
                            "ssh-form-cancel",
                            crate::i18n::pages::cancel(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.close_ssh_form(cx);
                            },
                        )),
                    ),
                )
                .child(
                    div().debug_selector(|| "ssh-form-save".into()).child(
                        button(
                            "ssh-form-save",
                            crate::i18n::pages::ssh_form_save(locale),
                            ButtonVariant::Primary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.save_ssh_profile_form(cx);
                            },
                        )),
                    ),
                ),
        );

        card = card.child(form);
    }

    div()
        .id("ssh-profiles-card")
        .debug_selector(|| "ssh-profiles-card".into())
        .w_full()
        .child(card)
}

/// Full-page Remote SSH server configuration screen.
pub fn remote_page(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let ssh_card = ssh_profiles_card(app, window, cx);

    div()
        .id("remote-page")
        .debug_selector(|| "remote-page".into())
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
                .child(crate::i18n::pages::remote_title(locale)),
        )
        .child(
            div()
                .text_color(theme.secondary)
                .child(crate::i18n::pages::remote_subtitle(locale)),
        )
        .child(
            div()
                .id("remote-content")
                .debug_selector(|| "remote-content".into())
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap(space::LG)
                .overflow_y_scroll()
                .child(ssh_card),
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

    let space_opt = app.watchdog_home_space.as_ref().or_else(|| {
        if cfg!(test) {
            (app.remote.is_none())
                .then_some(app.space.as_ref())
                .flatten()
        } else {
            None
        }
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
                        .child(div().text_color(theme.bright).child(format!(
                            "{}  ·  {}",
                            crate::i18n::pages::this_computer(locale),
                            home_display
                        ))),
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
                )
                .when(app.watchdog_analyzing, |col| {
                    let stage = crate::ai_stage_for_key(
                        &crate::AiEventKey::DiskCleanupLocal,
                        cx,
                    );
                    col.child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .child(div().text_color(theme.secondary).child(
                                crate::i18n::explore::ai_analyze_local_tooltip(
                                    locale,
                                ),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(space::XS)
                                    .debug_selector(|| {
                                        "watchdog-ai-stage".into()
                                    })
                                    .text_color(theme.accent)
                                    .child(spinning_loader(
                                        "watchdog-ai-stage-spin",
                                        theme.accent,
                                    ))
                                    .child(
                                        crate::i18n::explore::ai_stage_tooltip(
                                            stage, locale,
                                        ),
                                    ),
                            ),
                    )
                }),
        );

    let proposal_card = if let Some(ref prop) = app.ai_proposal {
        let item_bytes: u64 = prop.items.iter().map(|i| i.bytes).sum();
        let docker_bytes: u64 = match &app.local_docker {
            DockerProbeStatus::Available(summary) => prop
                .docker_targets
                .iter()
                .map(|target| match target {
                    DockerCleanTarget::BuildCache => summary
                        .build_cache_reclaimable_bytes
                        .max(summary.build_cache.reclaimable_bytes),
                    DockerCleanTarget::Image { id, reference, .. } => summary
                        .removable_images
                        .iter()
                        .find(|img| {
                            img.id == *id
                                && reference.as_deref().is_none_or(|req| {
                                    !img.dangling
                                        && format!(
                                            "{}:{}",
                                            img.repository, img.tag
                                        ) == req
                                })
                        })
                        .map_or(0, |img| {
                            if img.unique_bytes > 0 {
                                img.unique_bytes
                            } else {
                                img.size_bytes
                            }
                        }),
                    DockerCleanTarget::Container { id, .. } => summary
                        .stopped_containers
                        .iter()
                        .find(|c| c.id == *id)
                        .map_or(0, |c| c.size_bytes),
                })
                .sum(),
            DockerProbeStatus::NotInstalled
            | DockerProbeStatus::DaemonStopped
            | DockerProbeStatus::PermissionDenied { .. } => 0,
        };
        let total_candidate_bytes = item_bytes.saturating_add(docker_bytes);
        let total_candidate_count =
            prop.items.len() + prop.docker_targets.len();
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
                            total_candidate_count,
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
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.open_last_proposal(cx);
                                    }),
                                ),
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

    let ai_error_card = app.ai_last_error.as_ref().map(|err_summary| {
        div()
            .w_full()
            .debug_selector(|| "watchdog-ai-last-error".into())
            .child(
                dialog_popup(cx)
                    .w_full()
                    .border_color(theme.danger.opacity(0.45))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .gap(space::SM)
                            .child(dialog_title(
                                crate::i18n::pages::latest_ai_error_title(
                                    locale,
                                ),
                                cx,
                            ))
                            .when_some(app.ai_last_error_at_secs, |row, ts| {
                                row.child(
                                    div()
                                        .debug_selector(|| {
                                            "watchdog-ai-last-error-at".into()
                                        })
                                        .text_size(text::CAPTION)
                                        .text_color(theme.secondary)
                                        .child(format_checked_age(
                                            Some(ts),
                                            locale,
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .debug_selector(|| {
                                "watchdog-ai-last-error-text".into()
                            })
                            .text_size(text::BODY)
                            .text_color(theme.danger)
                            .child(err_summary.clone()),
                    )
                    .when(
                        app.cloud_payment_required
                            && app.ai_config.provider
                                == AiCliProvider::RuumCloud
                            && !app.cached_cloud_entitlement().is_some_and(
                                |e| {
                                    e.is_alipay_active_at(
                                        crate::state::current_unix_secs(),
                                    )
                                },
                            ),
                        |card| {
                            let now_secs = crate::state::current_unix_secs();
                            let cached_ent = app.cached_cloud_entitlement();
                            let upgrade_label = cloud_upgrade_action_label(
                                cached_ent.as_ref(),
                                locale,
                                now_secs,
                            );
                            card.child(
                                div()
                                    .pt(space::XS)
                                    .debug_selector(|| {
                                        "watchdog-cloud-upgrade-btn".into()
                                    })
                                    .child(
                                        button(
                                            "watchdog-cloud-upgrade-btn",
                                            upgrade_label,
                                            ButtonVariant::Primary,
                                            cx,
                                        )
                                        .on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.open_billing_modal(cx);
                                            },
                                        )),
                                    ),
                            )
                        },
                    ),
            )
    });

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
        .when_some(ai_error_card, gpui_kit::ParentElement::child)
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

fn general_settings_section(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> AnyElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let input_default_depth = app
        .setting_input_default_depth
        .as_ref()
        .expect("inputs initialized");
    let input_ssh_duration = app
        .setting_input_ssh_duration
        .as_ref()
        .expect("inputs initialized");

    let is_apparent = app.watchdog_draft.apparent_size;
    let is_hidden = app.watchdog_draft.include_hidden;
    let is_follow_appearance = app.watchdog_draft.follow_system_appearance;
    let is_close_to_background = app.watchdog_draft.close_to_background;
    let is_launch_at_login = app.watchdog_draft.launch_at_login;
    let is_tray_show_disk_space = app.watchdog_draft.tray_show_disk_space;
    let current_preset = app.watchdog_draft.power_preset;

    let entity_apparent = cx.entity().downgrade();
    let entity_hidden = entity_apparent.clone();
    let entity_appearance = entity_apparent.clone();
    let entity_close_bg = entity_apparent.clone();
    let entity_autostart = entity_apparent.clone();
    let entity_tray_space = entity_apparent.clone();

    let mut preset_buttons = div().flex().flex_row().flex_wrap().gap(space::SM);
    for preset in PowerEfficiency::ALL {
        if !preset.available(app.cpu_threads) {
            continue;
        }
        let is_selected = preset == current_preset;
        let id = ElementId::Name(
            format!("setting-power-preset-{}", preset.key()).into(),
        );
        let sel_id = format!("setting-power-preset-{}", preset.key());
        preset_buttons = preset_buttons.child(
            div().debug_selector(move || sel_id).child(
                button(
                    id,
                    preset.label_in(app.cpu_threads, locale),
                    if is_selected {
                        ButtonVariant::Primary
                    } else {
                        ButtonVariant::Secondary
                    },
                    cx,
                )
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.watchdog_draft.power_preset = preset;
                        cx.notify();
                    },
                )),
            ),
        );
    }

    div()
        .id("setting-general-section")
        .debug_selector(|| "setting-general-section".into())
        .w_full()
        .child(
            dialog_popup(cx)
                .w_full()
                .child(dialog_title(
                    crate::i18n::pages::general_settings_title(locale),
                    cx,
                ))
        .child(dialog_description(
            crate::i18n::pages::general_settings_desc(locale),
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::MD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::general_power_preset(locale),
                        ))
                        .child(preset_buttons),
                )
                .child(setting_input_field(
                    "setting-default-depth-input",
                    crate::i18n::pages::general_default_depth(locale),
                    input_default_depth,
                    theme.secondary,
                    window,
                    cx,
                ))
                .child(
                    div()
                        .debug_selector(|| "setting-apparent-size".into())
                        .child(
                            checkbox(
                                "setting-apparent-size",
                                crate::i18n::pages::general_apparent_size(
                                    locale,
                                ),
                                if is_apparent {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_apparent.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_draft.apparent_size =
                                                !this
                                                    .watchdog_draft
                                                    .apparent_size;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-include-hidden".into())
                        .child(
                            checkbox(
                                "setting-include-hidden",
                                crate::i18n::pages::general_include_hidden(
                                    locale,
                                ),
                                if is_hidden {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ =
                                        entity_hidden.update(cx, |this, cx| {
                                            this.watchdog_draft
                                                .include_hidden = !this
                                                .watchdog_draft
                                                .include_hidden;
                                            cx.notify();
                                        });
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-follow-appearance".into())
                        .child(
                            checkbox(
                                "setting-follow-appearance",
                                crate::i18n::pages::general_follow_appearance(
                                    locale,
                                ),
                                if is_follow_appearance {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_appearance.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_draft
                                                .follow_system_appearance =
                                                !this
                                                    .watchdog_draft
                                                    .follow_system_appearance;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-close-to-background".into())
                        .child(
                            checkbox(
                                "setting-close-to-background",
                                crate::i18n::pages::general_close_to_background(
                                    locale,
                                ),
                                if is_close_to_background {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_close_bg.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_draft
                                                .close_to_background = !this
                                                .watchdog_draft
                                                .close_to_background;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-launch-at-login".into())
                        .child(
                            checkbox(
                                "setting-launch-at-login",
                                crate::i18n::pages::general_launch_at_login(
                                    locale,
                                ),
                                if is_launch_at_login {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ = entity_autostart.update(
                                        cx,
                                        |this, cx| {
                                            this.watchdog_draft
                                                .launch_at_login = !this
                                                .watchdog_draft
                                                .launch_at_login;
                                            cx.notify();
                                        },
                                    );
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-tray-disk-space".into())
                        .child(
                            checkbox(
                                "setting-tray-disk-space",
                                crate::i18n::pages::general_tray_show_disk_space(
                                    locale,
                                ),
                                if is_tray_show_disk_space {
                                    CheckboxState::Checked
                                } else {
                                    CheckboxState::Unchecked
                                },
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(move |_, _, _, cx| {
                                let _ = entity_tray_space.update(
                                    cx,
                                    |this, cx| {
                                        this.watchdog_draft
                                            .tray_show_disk_space = !this
                                            .watchdog_draft
                                            .tray_show_disk_space;
                                        cx.notify();
                                    },
                                );
                            }),
                        ),
                )
                .child(setting_input_field(
                    "setting-ssh-duration-input",
                    crate::i18n::pages::general_ssh_lease_minutes(locale),
                    input_ssh_duration,
                    theme.secondary,
                    window,
                    cx,
                )),
        ),
        )
        .into_any_element()
}

fn watchdog_settings_section(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> AnyElement {
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

    let is_enabled = app.watchdog_draft.enabled;
    let entity_watchdog = cx.entity().downgrade();

    dialog_popup(cx)
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
        )
        .into_any_element()
}

const WEBHOOK_HEADERS_EXAMPLE: &str =
    r#"{"Authorization": "Bearer <token>", "X-Source": "ruum"}"#;
const WEBHOOK_PAYLOAD_EXAMPLE: &str = r#"{
  "event": "ruum.disk.low_space",
  "timestamp": "2026-01-01T00:00:00Z",
  "host": { "hostname": "srv-1", "machine_id": "...", "platform": "linux" },
  "mount": {
    "path": "/home",
    "available_bytes": 1073741824,
    "total_bytes": 107374182400,
    "threshold_bytes": 5368709120
  }
}"#;
const WEBHOOK_CLI_EXAMPLE: &str = "ruum config webhook add --name ops --url https://hooks.example.com/ruum --enable";

fn webhook_settings_section(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> AnyElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;

    let mut guide_card = dialog_popup(cx).w_full().child(
        div()
            .id("setting-webhook-guide-toggle")
            .debug_selector(|| "setting-webhook-guide-toggle".into())
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(space::SM)
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_webhook_guide(cx);
            }))
            .child(dialog_title(
                crate::i18n::pages::webhook_guide_title(locale),
                cx,
            ))
            .child(
                gpui_omarchy::icon(if app.webhook_guide_open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(icon::SM)
                .text_color(theme.secondary),
            ),
    );
    if app.webhook_guide_open {
        guide_card = guide_card.child(
            div()
                .id("setting-webhook-guide")
                .debug_selector(|| "setting-webhook-guide".into())
                .flex()
                .flex_col()
                .gap(space::SM)
                .p(space::MD)
                .rounded(space::XS)
                .border_1()
                .border_color(theme.divider())
                .bg(theme.normal_fill())
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.bright)
                        .child(crate::i18n::pages::webhook_guide_flow(locale)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XXS)
                        .child(
                            div()
                                .text_size(text::CAPTION)
                                .text_color(theme.secondary)
                                .child(
                                    crate::i18n::pages::webhook_guide_headers_label(
                                        locale,
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .p(space::XS)
                                .rounded(space::XXS)
                                .bg(theme.background)
                                .font_family(theme.font.clone())
                                .text_size(text::CAPTION)
                                .text_color(theme.bright)
                                .child(WEBHOOK_HEADERS_EXAMPLE),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XXS)
                        .child(
                            div()
                                .text_size(text::CAPTION)
                                .text_color(theme.secondary)
                                .child(
                                    crate::i18n::pages::webhook_guide_payload_label(
                                        locale,
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .p(space::XS)
                                .rounded(space::XXS)
                                .bg(theme.background)
                                .font_family(theme.font.clone())
                                .text_size(text::CAPTION)
                                .text_color(theme.bright)
                                .child(WEBHOOK_PAYLOAD_EXAMPLE),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XXS)
                        .child(
                            div()
                                .text_size(text::CAPTION)
                                .text_color(theme.secondary)
                                .child(
                                    crate::i18n::pages::webhook_guide_cli_label(
                                        locale,
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .p(space::XS)
                                .rounded(space::XXS)
                                .bg(theme.background)
                                .font_family(theme.font.clone())
                                .text_size(text::CAPTION)
                                .text_color(theme.bright)
                                .child(WEBHOOK_CLI_EXAMPLE),
                        ),
                ),
        );
    }

    let mut list_card = dialog_popup(cx).w_full().child(
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
                    .child(dialog_title(
                        crate::i18n::pages::webhook_title(locale),
                        cx,
                    ))
                    .child(dialog_description(
                        crate::i18n::pages::webhook_desc(locale),
                        cx,
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "setting-webhook-add-btn".into())
                    .child(with_tooltip(
                        gpui_omarchy::icon_button(
                            "setting-webhook-add-btn",
                            IconName::Plus,
                            crate::i18n::pages::webhook_add_tooltip(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.open_add_webhook_form(cx);
                            },
                        )),
                        crate::i18n::pages::webhook_add_tooltip(locale),
                    )),
            ),
    );

    if app.webhooks.is_empty() && !app.webhook_form_open {
        list_card = list_card.child(dialog_description(
            crate::i18n::pages::webhook_none(locale),
            cx,
        ));
    } else if !app.webhooks.is_empty() {
        let mut rows = div().flex().flex_col().gap(space::SM);
        for (i, hook) in app.webhooks.iter().enumerate() {
            let is_testing = app.webhook_testing_ids.contains(&hook.id);
            let id_toggle = hook.id.clone();
            let id_test = hook.id.clone();
            let id_edit = hook.id.clone();
            let id_delete = hook.id.clone();

            let toggle_badge = div()
                .id(ElementId::Name(
                    format!("setting-webhook-toggle-{i}").into(),
                ))
                .debug_selector(move || format!("setting-webhook-toggle-{i}"))
                .cursor_pointer()
                .px(space::XS)
                .py(space::XXS)
                .rounded(space::XS)
                .bg(if hook.enabled {
                    theme.accent.opacity(0.18)
                } else {
                    theme.divider()
                })
                .text_size(text::CAPTION)
                .text_color(if hook.enabled {
                    theme.accent
                } else {
                    theme.secondary
                })
                .child(if hook.enabled {
                    crate::i18n::pages::webhook_enabled_badge(locale)
                } else {
                    crate::i18n::pages::webhook_disabled_badge(locale)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_webhook_enabled(&id_toggle, cx);
                }));

            let test_btn = div()
                .debug_selector(move || format!("setting-webhook-test-{i}"))
                .child(with_tooltip(
                    if is_testing {
                        button(
                            ElementId::Name(
                                format!("setting-webhook-test-{i}").into(),
                            ),
                            "",
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .accessibility_label(
                            crate::i18n::pages::webhook_test_tooltip(locale),
                        )
                        .p(space::SM)
                        .justify_center()
                        .child(spinning_loader(
                            format!("setting-webhook-test-spin-{i}"),
                            theme.foreground,
                        ))
                    } else {
                        gpui_omarchy::icon_button(
                            ElementId::Name(
                                format!("setting-webhook-test-{i}").into(),
                            ),
                            IconName::Play,
                            crate::i18n::pages::webhook_test_tooltip(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                    }
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.test_webhook_endpoint_now(&id_test, cx);
                        },
                    )),
                    crate::i18n::pages::webhook_test_tooltip(locale),
                ));

            let edit_btn = div()
                .debug_selector(move || format!("setting-webhook-edit-{i}"))
                .child(with_tooltip(
                    button(
                        ElementId::Name(
                            format!("setting-webhook-edit-{i}").into(),
                        ),
                        "",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .accessibility_label(crate::i18n::pages::edit_ssh_server(
                        locale,
                    ))
                    .p(space::SM)
                    .justify_center()
                    .child(pencil_icon(theme.foreground))
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.open_edit_webhook_form(&id_edit, cx);
                        },
                    )),
                    crate::i18n::pages::edit_ssh_server(locale),
                ));

            let delete_btn = div()
                .debug_selector(move || format!("setting-webhook-delete-{i}"))
                .child(with_tooltip(
                    button(
                        ElementId::Name(
                            format!("setting-webhook-delete-{i}").into(),
                        ),
                        "",
                        ButtonVariant::Danger,
                        cx,
                    )
                    .accessibility_label(crate::i18n::pages::delete_ssh_server(
                        locale,
                    ))
                    .p(space::SM)
                    .justify_center()
                    .child(trash_icon(theme.danger))
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.delete_webhook_endpoint(&id_delete, cx);
                        },
                    )),
                    crate::i18n::pages::delete_ssh_server(locale),
                ));

            rows = rows.child(
                div()
                    .id(ElementId::Name(
                        format!("setting-webhook-row-{i}").into(),
                    ))
                    .debug_selector(move || format!("setting-webhook-row-{i}"))
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .p(space::MD)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.divider())
                    .bg(theme.normal_fill())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(space::SM)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(space::SM)
                                    .child(
                                        div()
                                            .text_size(text::BODY)
                                            .text_color(theme.bright)
                                            .child(hook.name.clone()),
                                    )
                                    .child(toggle_badge),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(space::XS)
                                    .child(test_btn)
                                    .child(edit_btn)
                                    .child(delete_btn),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(space::MD)
                            .text_size(text::CAPTION)
                            .text_color(theme.secondary)
                            .child(hook.url.clone())
                            .child(
                                crate::i18n::pages::webhook_timeout_summary(
                                    hook.timeout_secs,
                                    locale,
                                ),
                            ),
                    ),
            );
        }
        list_card = list_card.child(rows);
    }

    if app.webhook_form_open
        && let (
            Some(input_name),
            Some(input_url),
            Some(input_timeout),
            Some(input_headers),
        ) = (
            app.webhook_input_name.as_ref(),
            app.setting_input_webhook_url.as_ref(),
            app.setting_input_webhook_timeout.as_ref(),
            app.setting_input_webhook_headers.as_ref(),
        )
    {
        let form_title = if app.webhook_form_editing_id.is_some() {
            crate::i18n::pages::webhook_form_edit_title(locale)
        } else {
            crate::i18n::pages::webhook_form_add_title(locale)
        };
        let entity_enabled = cx.entity().downgrade();
        let mut form = div()
            .id("setting-webhook-form")
            .debug_selector(|| "setting-webhook-form".into())
            .flex()
            .flex_col()
            .gap(space::SM)
            .p(space::MD)
            .rounded(space::XS)
            .border_1()
            .border_color(theme.control_border())
            .bg(theme.normal_fill())
            .child(dialog_title(form_title, cx))
            .child(setting_input_field(
                "setting-webhook-name-input",
                crate::i18n::pages::webhook_name_label(locale),
                input_name,
                theme.secondary,
                window,
                cx,
            ))
            .child(setting_input_field(
                "setting-webhook-url-input",
                crate::i18n::pages::webhook_url(locale),
                input_url,
                theme.secondary,
                window,
                cx,
            ))
            .child(setting_input_field(
                "setting-webhook-timeout-input",
                crate::i18n::pages::webhook_timeout_secs(locale),
                input_timeout,
                theme.secondary,
                window,
                cx,
            ))
            .child(setting_input_field(
                "setting-webhook-headers-input",
                crate::i18n::pages::webhook_headers_json(locale),
                input_headers,
                theme.secondary,
                window,
                cx,
            ))
            .child(
                div()
                    .debug_selector(|| "setting-webhook-enabled".into())
                    .child(
                        checkbox(
                            "setting-webhook-enabled",
                            crate::i18n::pages::enable_webhook(locale),
                            if app.webhook_form_enabled {
                                CheckboxState::Checked
                            } else {
                                CheckboxState::Unchecked
                            },
                            cx,
                        )
                        .tab_stop(false)
                        .on_change(
                            move |_, _, _, cx| {
                                let _ =
                                    entity_enabled.update(cx, |this, cx| {
                                        this.webhook_form_enabled =
                                            !this.webhook_form_enabled;
                                        cx.notify();
                                    });
                            },
                        ),
                    ),
            );
        if let Some(ref err) = app.webhook_form_error {
            form = form.child(
                div()
                    .debug_selector(|| "setting-webhook-form-error".into())
                    .text_size(text::CAPTION)
                    .text_color(theme.danger)
                    .child(err.clone()),
            );
        }
        form = form.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::SM)
                .child(
                    div()
                        .debug_selector(|| "setting-webhook-cancel".into())
                        .child(
                            button(
                                "setting-webhook-cancel",
                                crate::i18n::pages::cancel(locale),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.close_webhook_form(cx);
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "setting-webhook-save".into())
                        .child(
                            button(
                                "setting-webhook-save",
                                crate::i18n::pages::webhook_save(locale),
                                ButtonVariant::Primary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.save_webhook_form(cx);
                                },
                            )),
                        ),
                ),
        );
        list_card = list_card.child(form);
    }

    div()
        .id("setting-webhook-section")
        .debug_selector(|| "setting-webhook-section".into())
        .flex()
        .flex_col()
        .gap(space::LG)
        .w_full()
        .child(guide_card)
        .child(list_card)
        .into_any_element()
}

fn ai_settings_section(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> AnyElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
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
    let input_api_base_url = app
        .watchdog_input_api_base_url
        .as_ref()
        .expect("inputs initialized");
    let input_api_key = app
        .watchdog_input_api_key
        .as_ref()
        .expect("inputs initialized");

    let is_auto_analyze = app.watchdog_draft.ai_auto_analyze;
    let current_provider = app.watchdog_draft.ai_provider;
    let current_protocol = app.watchdog_draft.api_protocol;
    let is_api = current_provider == AiCliProvider::Api;
    let is_ruum_cloud = current_provider == AiCliProvider::RuumCloud;
    let entity_auto_analyze = cx.entity().downgrade();

    let mut provider_buttons =
        div().flex().flex_row().flex_wrap().gap(space::SM);
    for provider in AiCliProvider::ALL {
        let is_selected = provider == current_provider;
        let id = match provider {
            AiCliProvider::RuumCloud => "provider-ruum-cloud",
            AiCliProvider::Agy => "provider-agy",
            AiCliProvider::Pi => "provider-pi",
            AiCliProvider::Codex => "provider-codex",
            AiCliProvider::Custom => "provider-custom",
            AiCliProvider::Api => "provider-api",
        };
        provider_buttons = provider_buttons.child(
            div().debug_selector(move || id.into()).child(
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
                .on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.set_watchdog_provider(provider, cx);
                    },
                )),
            ),
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
                            this.toggle_watchdog_ai_auto_analyze(cx);
                        });
                    }),
                ),
        )
        .child(provider_buttons);

    let show_custom_cmd = !is_api
        && !is_ruum_cloud
        && (current_provider == AiCliProvider::Custom
            || !app.ai_config.custom_command.is_empty()
            || !input_custom_cmd.read(cx).value().trim().is_empty());
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

    if is_ruum_cloud {
        ai_section =
            ai_section.child(ruum_cloud_settings_card(app, window, cx));
        return ai_section.into_any_element();
    }

    if is_api {
        let mut protocol_buttons =
            div().flex().flex_row().flex_wrap().gap(space::SM);
        for protocol in ApiProtocol::ALL {
            let is_selected = protocol == current_protocol;
            let id = match protocol {
                ApiProtocol::OpenAi => "setting-api-protocol-openai",
                ApiProtocol::Anthropic => "setting-api-protocol-anthropic",
            };
            protocol_buttons = protocol_buttons.child(
                div().debug_selector(move || id.into()).child(
                    button(
                        id,
                        protocol.label_in(locale),
                        if is_selected {
                            ButtonVariant::Primary
                        } else {
                            ButtonVariant::Secondary
                        },
                        cx,
                    )
                    .on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.set_watchdog_api_protocol(
                                protocol, window, cx,
                            );
                        },
                    )),
                ),
            );
        }

        let has_saved_hint = match app.api_key_session_known {
            Some(known) => known,
            None => app.cred_store.as_ref().is_some_and(
                ruum_core::credentials::LazyCredentialStore::has_encrypted_file,
            ) || ruum_core::ai_cli::env_api_key_present(),
        };
        let key_hint = if has_saved_hint {
            crate::i18n::pages::api_key_keep_hint(locale)
        } else {
            crate::i18n::pages::api_key_empty_hint(locale)
        };

        ai_section = ai_section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div().text_color(theme.secondary).child(
                            crate::i18n::pages::api_protocol_label(locale),
                        ),
                    )
                    .child(protocol_buttons),
            )
            .child(setting_input_field(
                "setting-api-base-url-input",
                crate::i18n::pages::api_base_url_label(locale),
                input_api_base_url,
                theme.secondary,
                window,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(space::SM)
                            .child(div().flex_1().child(setting_input_field(
                                "setting-api-key-input",
                                crate::i18n::pages::api_key_label(locale),
                                input_api_key,
                                theme.secondary,
                                window,
                                cx,
                            )))
                            .child(
                                div()
                                    .debug_selector(|| {
                                        "setting-clear-api-key".into()
                                    })
                                    .child(
                                        button(
                                            "setting-clear-api-key",
                                            crate::i18n::pages::clear_api_key(
                                                locale,
                                            ),
                                            ButtonVariant::Secondary,
                                            cx,
                                        )
                                        .on_click(cx.listener(
                                            |this, _, window, cx| {
                                                this.clear_watchdog_api_key(
                                                    window, cx,
                                                );
                                            },
                                        )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.secondary)
                            .child(key_hint),
                    ),
            );
    } else {
        ai_section = ai_section.child(setting_input_field(
            "setting-extra-args-input",
            crate::i18n::pages::custom_extra_args(locale),
            input_extra_args,
            theme.secondary,
            window,
            cx,
        ));
    }

    let test_btn_label = if is_api {
        crate::i18n::pages::test_api_connection(locale)
    } else {
        crate::i18n::pages::test_cli_connection(locale)
    };

    ai_section = ai_section
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
                        action_button_with_spinner(
                            "setting-test-cli",
                            "setting-test-cli-spin",
                            test_btn_label,
                            app.watchdog_testing,
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
                            action_button_with_spinner(
                                "setting-fetch-models",
                                "setting-fetch-models-spin",
                                crate::i18n::pages::fetch_models(locale),
                                app.watchdog_fetching_models,
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
            .max_h(size::CLOUD_MODELS_MAX_H)
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
                            format!("setting-model-spin-{index}"),
                            theme.accent,
                        )
                        .into_any_element()
                    } else if let Some(res) = speed_opt {
                        div()
                            .text_size(text::CAPTION)
                            .text_color(speed_result_color(res, &theme))
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

            let select_tooltip = if is_selected {
                crate::i18n::pages::selected(locale)
            } else {
                crate::i18n::pages::select(locale)
            };
            let select_control = div()
                .debug_selector(move || format!("setting-model-select-{index}"))
                .child(with_tooltip(
                    button(
                        ElementId::Name(
                            format!("setting-model-select-{index}").into(),
                        ),
                        "",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .accessibility_label(select_tooltip)
                    .p(space::XS)
                    .justify_center()
                    .when(is_selected, |b| b.bg(theme.accent.opacity(0.18)))
                    .child(if is_selected {
                        gpui_omarchy::icon(IconName::CircleCheck)
                            .size(icon::MD)
                            .text_color(theme.accent)
                            .into_any_element()
                    } else {
                        circle_icon(theme.secondary).into_any_element()
                    })
                    .on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.select_watchdog_model(
                                &select_btn_id,
                                window,
                                cx,
                            );
                        },
                    )),
                    select_tooltip,
                ));

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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if any_benchmarking {
                            this.benchmark_cancel(cx);
                        } else {
                            this.benchmark_all_watchdog_models(cx);
                        }
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

    ai_section.into_any_element()
}

pub fn cloud_upgrade_action_label(
    entitlement: Option<&ruum_core::cloud_client::EntitlementSnapshot>,
    locale: crate::i18n::Locale,
    now_secs: u64,
) -> &'static str {
    if entitlement.is_some_and(|e| e.has_expired_alipay_history_at(now_secs)) {
        crate::i18n::pages::cloud_renew_pass(locale)
    } else {
        crate::i18n::pages::cloud_upgrade_pass(locale)
    }
}

pub fn cloud_entitlement_badge_label(
    entitlement: Option<&ruum_core::cloud_client::EntitlementSnapshot>,
    locale: crate::i18n::Locale,
    now_secs: u64,
) -> String {
    match entitlement {
        None => {
            crate::i18n::pages::cloud_status_not_activated(locale).to_owned()
        }
        Some(ent) if ent.is_effectively_paid_at(now_secs) => {
            let until = if ent.is_alipay_active_at(now_secs) {
                ent.alipay_valid_until_secs.or(ent.valid_until_secs)
            } else {
                ent.grant_valid_until_secs
                    .filter(|&u| u > now_secs)
                    .or(ent.valid_until_secs)
            };
            let date =
                until.map_or_else(|| "-".to_owned(), format_unix_date_ymd);
            crate::i18n::pages::cloud_status_paid_until(
                &date,
                &ent.source,
                locale,
            )
        }
        Some(ent)
            if ent.trial_remaining > 0
                && !ent.decision.eq_ignore_ascii_case("DENY") =>
        {
            crate::i18n::pages::cloud_status_trial(ent.trial_remaining, locale)
        }
        Some(_) => crate::i18n::pages::cloud_status_expired(locale).to_owned(),
    }
}

fn cloud_entitlement_status_row(
    entitlement: Option<&ruum_core::cloud_client::EntitlementSnapshot>,
    locale: crate::i18n::Locale,
    theme: &gpui_omarchy::Theme,
    now_secs: u64,
) -> Div {
    let badge_text =
        cloud_entitlement_badge_label(entitlement, locale, now_secs);
    let badge_color = match entitlement {
        None => theme.secondary,
        Some(ent) if ent.is_effectively_paid_at(now_secs) => theme.success,
        Some(ent)
            if ent.trial_remaining > 0
                && !ent.decision.eq_ignore_ascii_case("DENY") =>
        {
            theme.accent
        }
        Some(_) => theme.danger,
    };

    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(space::SM)
        .child(
            div()
                .id("cloud-entitlement-badge")
                .debug_selector(|| "cloud-entitlement-badge".into())
                .px(space::SM)
                .py(space::XS)
                .rounded(space::XS)
                .border_1()
                .border_color(badge_color.opacity(0.45))
                .bg(badge_color.opacity(0.14))
                .text_size(text::CAPTION)
                .text_color(badge_color)
                .child(badge_text),
        );

    if let Some(ent) = entitlement {
        let fup_zone = ent.fup_zone.trim();
        let fup_color = match fup_zone.to_ascii_lowercase().as_str() {
            "yellow" => theme.warning,
            "red" => theme.danger,
            _ => theme.success,
        };
        let fup_label = crate::i18n::pages::cloud_fup_label(fup_zone, locale);
        row = row.child(
            div()
                .id("cloud-fup-pill")
                .debug_selector(|| "cloud-fup-pill".into())
                .px(space::SM)
                .py(space::XS)
                .rounded(space::XS)
                .border_1()
                .border_color(fup_color.opacity(0.45))
                .bg(fup_color.opacity(0.14))
                .text_size(text::CAPTION)
                .text_color(fup_color)
                .child(fup_label),
        );
        if ent.is_alipay_active_at(now_secs) {
            row = row.child(
                div()
                    .id("cloud-alipay-active-hint")
                    .debug_selector(|| "cloud-alipay-active-hint".into())
                    .text_size(text::CAPTION)
                    .text_color(theme.secondary)
                    .child(crate::i18n::pages::cloud_alipay_active_renew_hint(
                        locale,
                    )),
            );
        }
    }

    row
}

const fn speed_result_color(
    res: &ruum_core::ai_cli::ModelSpeedResult,
    theme: &gpui_omarchy::Theme,
) -> Hsla {
    if !res.ok || res.elapsed_ms >= 4_000 {
        theme.danger
    } else if res.elapsed_ms >= 1_500 {
        theme.warning
    } else {
        theme.success
    }
}

fn cloud_virtual_model_row(
    app: &Ruum,
    index: usize,
    model: &ruum_core::cloud_client::CloudVirtualModel,
    cx: &Context<'_, Ruum>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let is_selected = app.watchdog_draft.model.as_deref()
        == Some(model.id.as_str())
        && model.available;
    let title = model
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&model.id)
        .to_owned();
    let tier_label = if model.tier.trim().is_empty() {
        "cloud".to_owned()
    } else {
        model.tier.trim().to_owned()
    };
    let desc = model
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let model_id_pick = model.id.clone();
    let model_id_bench = model.id.clone();
    let is_benchmarking = app.watchdog_benchmarking_models.contains(&model.id);
    let speed_opt = app
        .watchdog_speed_results
        .iter()
        .find(|r| r.model == model.id);

    let mut info_col = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(space::XXS)
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(space::SM)
                .child(
                    div()
                        .text_color(if is_selected {
                            theme.accent
                        } else if model.available {
                            theme.bright
                        } else {
                            theme.secondary
                        })
                        .child(title),
                )
                .child(
                    div()
                        .px(space::XS)
                        .py(space::XXS)
                        .rounded(space::XXS)
                        .bg(theme.normal_fill())
                        .border_1()
                        .border_color(theme.divider())
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(tier_label),
                ),
        );

    if let Some(description) = desc {
        info_col = info_col.child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(description),
        );
    }

    let mut right_col = div().flex().flex_row().items_center().gap(space::SM);

    if model.available {
        let speed_btn = div()
            .id(ElementId::Name(
                format!("setting-model-speed-{index}").into(),
            ))
            .debug_selector(move || format!("setting-model-speed-{index}"))
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
                this.benchmark_watchdog_model(Some(model_id_bench.clone()), cx);
            }))
            .child(if is_benchmarking {
                spinning_loader(
                    format!("setting-model-spin-{index}"),
                    theme.accent,
                )
                .into_any_element()
            } else if let Some(res) = speed_opt {
                div()
                    .text_size(text::CAPTION)
                    .text_color(speed_result_color(res, &theme))
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
            });
        right_col = right_col.child(speed_btn);
    }

    if model.available {
        let select_id = model.id.clone();
        right_col = right_col.child(
            div()
                .debug_selector(move || format!("cloud-model-select-{index}"))
                .child(
                    button(
                        ElementId::Name(
                            format!("cloud-model-select-{index}").into(),
                        ),
                        "",
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .accessibility_label(if is_selected {
                        crate::i18n::pages::selected(locale)
                    } else {
                        crate::i18n::pages::select(locale)
                    })
                    .p(space::XS)
                    .justify_center()
                    .when(is_selected, |b| b.bg(theme.accent.opacity(0.18)))
                    .child(if is_selected {
                        gpui_omarchy::icon(IconName::CircleCheck)
                            .size(icon::MD)
                            .text_color(theme.accent)
                            .into_any_element()
                    } else {
                        circle_icon(theme.secondary).into_any_element()
                    })
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.select_cloud_model(&select_id, cx);
                        },
                    )),
                ),
        );
    } else {
        right_col = right_col.child(
            div()
                .px(space::XS)
                .py(space::XXS)
                .rounded(space::XXS)
                .border_1()
                .border_color(theme.warning.opacity(0.45))
                .bg(theme.warning.opacity(0.12))
                .text_size(text::CAPTION)
                .text_color(theme.warning)
                .child(crate::i18n::pages::cloud_model_locked_badge(locale)),
        );
    }

    let mut row = div()
        .id(ElementId::Name(format!("cloud-model-row-{index}").into()))
        .debug_selector(move || format!("cloud-model-row-{index}"))
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
            theme.normal_fill()
        })
        .border_1()
        .border_color(if is_selected {
            theme.accent.opacity(0.45)
        } else {
            theme.divider()
        })
        .child(info_col)
        .child(right_col);

    if model.available {
        row = row.cursor_pointer().on_click(cx.listener(
            move |this, _, _, cx| {
                this.select_cloud_model(&model_id_pick, cx);
            },
        ));
    }
    row
}

fn cloud_auto_model_row(app: &Ruum, cx: &Context<'_, Ruum>) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let is_selected = app.watchdog_draft.model.is_none();

    let info_col = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(space::XXS)
        .child(
            div()
                .text_color(if is_selected {
                    theme.accent
                } else {
                    theme.bright
                })
                .child(crate::i18n::pages::cloud_model_auto_title(locale)),
        )
        .child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::cloud_model_auto_desc(locale)),
        );

    let select_btn = div()
        .debug_selector(|| "cloud-model-select-auto".into())
        .child(
            button("cloud-model-select-auto", "", ButtonVariant::Secondary, cx)
                .accessibility_label(if is_selected {
                    crate::i18n::pages::selected(locale)
                } else {
                    crate::i18n::pages::select(locale)
                })
                .p(space::XS)
                .justify_center()
                .when(is_selected, |b| b.bg(theme.accent.opacity(0.18)))
                .child(if is_selected {
                    gpui_omarchy::icon(IconName::CircleCheck)
                        .size(icon::MD)
                        .text_color(theme.accent)
                        .into_any_element()
                } else {
                    circle_icon(theme.secondary).into_any_element()
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.select_cloud_model_auto(cx);
                })),
        );

    div()
        .id("cloud-model-row-auto")
        .debug_selector(|| "cloud-model-row-auto".into())
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
            theme.normal_fill()
        })
        .border_1()
        .border_color(if is_selected {
            theme.accent.opacity(0.45)
        } else {
            theme.divider()
        })
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.select_cloud_model_auto(cx);
        }))
        .child(info_col)
        .child(select_btn)
}

pub struct RedeemControlIds {
    pub control: &'static str,
    pub input: &'static str,
    pub submit: &'static str,
    pub spinner: &'static str,
    pub feedback: &'static str,
}

pub const SETTINGS_REDEEM_IDS: RedeemControlIds = RedeemControlIds {
    control: "cloud-settings-redeem-control",
    input: "cloud-settings-redeem-input",
    submit: "cloud-settings-redeem-btn",
    spinner: "cloud-settings-redeem-spin",
    feedback: "cloud-settings-redeem-feedback",
};

pub fn redeem_code_control(
    ids: &'static RedeemControlIds,
    app: &Ruum,
    input: &Entity<InputState>,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let submitting = app.redeem_submitting;
    let submit_btn = action_button_with_spinner(
        ids.submit,
        ids.spinner,
        crate::i18n::pages::cloud_redeem_submit(locale),
        submitting,
        ButtonVariant::Secondary,
        cx,
    )
    .disabled(submitting)
    .when(!submitting, |btn| {
        btn.on_click(cx.listener(move |this, _, _, cx| {
            this.submit_redeem_code(cx);
        }))
    });

    div()
        .id(ids.control)
        .debug_selector(move || ids.control.into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::cloud_redeem_title(locale)),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::SM)
                .child(
                    div().flex_1().min_w_0().child(
                        gpui_omarchy::input(ids.input, input, window, cx)
                            .debug_selector(move || ids.input.into()),
                    ),
                )
                .child(
                    div()
                        .debug_selector(move || ids.submit.into())
                        .child(submit_btn),
                ),
        )
        .when_some(app.cloud_redeem_feedback.as_ref(), |col, (msg, is_err)| {
            col.child(
                div()
                    .id(ids.feedback)
                    .debug_selector(move || ids.feedback.into())
                    .text_size(text::CAPTION)
                    .text_color(if *is_err {
                        theme.danger
                    } else {
                        theme.secondary
                    })
                    .child(msg.clone()),
            )
        })
}

fn ruum_cloud_settings_card(
    app: &Ruum,
    window: &Window,
    cx: &mut Context<'_, Ruum>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let now_secs = crate::state::current_unix_secs();
    let entitlement = app.cached_cloud_entitlement();
    let models = app.cached_cloud_models();
    let alipay_active = entitlement
        .as_ref()
        .is_some_and(|e| e.is_alipay_active_at(now_secs));
    let upgrade_label =
        cloud_upgrade_action_label(entitlement.as_ref(), locale, now_secs);
    let redeem_input = app
        .cloud_redeem_input_settings
        .as_ref()
        .expect("settings redeem input initialized");

    let mut card = div()
        .id("cloud-settings-card")
        .debug_selector(|| "cloud-settings-card".into())
        .flex()
        .flex_col()
        .gap(space::MD)
        .p(space::MD)
        .rounded(space::XS)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.normal_fill())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::XXS)
                .child(
                    div()
                        .text_size(text::TITLE)
                        .text_color(theme.bright)
                        .child(crate::i18n::pages::cloud_card_title(locale)),
                )
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(crate::i18n::pages::cloud_card_desc(locale)),
                ),
        )
        .child(cloud_entitlement_status_row(
            entitlement.as_ref(),
            locale,
            &theme,
            now_secs,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(space::SM)
                .when(!alipay_active, |row| {
                    row.child(
                        div()
                            .debug_selector(|| "cloud-upgrade-btn".into())
                            .child(
                                button(
                                    "cloud-upgrade-btn",
                                    upgrade_label,
                                    ButtonVariant::Primary,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.open_billing_modal(cx);
                                    }),
                                ),
                            ),
                    )
                })
                .child(
                    div()
                        .debug_selector(|| {
                            "cloud-refresh-entitlement-btn".into()
                        })
                        .child(
                            action_button_with_spinner(
                                "cloud-refresh-entitlement-btn",
                                "cloud-refresh-entitlement-spin",
                                crate::i18n::pages::cloud_refresh_entitlement(
                                    locale,
                                ),
                                app.cloud_refreshing,
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.refresh_cloud_entitlement(cx);
                                },
                            )),
                        ),
                )
                .child(
                    div().debug_selector(|| "setting-test-cli".into()).child(
                        action_button_with_spinner(
                            "setting-test-cli",
                            "setting-test-cli-spin",
                            crate::i18n::pages::cloud_test_connection(locale),
                            app.watchdog_testing,
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
                        .debug_selector(|| "cloud-fetch-models-btn".into())
                        .child(
                            action_button_with_spinner(
                                "cloud-fetch-models-btn",
                                "cloud-fetch-models-spin",
                                crate::i18n::pages::cloud_fetch_models_btn(
                                    locale,
                                ),
                                app.cloud_fetching_models,
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .disabled(app.cloud_fetching_models)
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.fetch_cloud_models(cx);
                                },
                            )),
                        ),
                ),
        );

    card = card.child(redeem_code_control(
        &SETTINGS_REDEEM_IDS,
        app,
        redeem_input,
        window,
        cx,
    ));

    let mut list = div()
        .id("cloud-virtual-models-list")
        .debug_selector(|| "cloud-virtual-models-list".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .max_h(size::CLOUD_MODELS_MAX_H)
        .overflow_y_scroll()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(cloud_auto_model_row(app, cx));
    for (index, model) in models.iter().enumerate() {
        list = list.child(cloud_virtual_model_row(app, index, model, cx));
    }

    if !models.is_empty() {
        let has_available = models.iter().any(|m| m.available);
        let any_benchmarking = !app.watchdog_benchmarking_models.is_empty();
        let mut header_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(space::SM)
            .child(
                div()
                    .text_size(text::CAPTION)
                    .text_color(theme.secondary)
                    .child(crate::i18n::pages::cloud_virtual_models_title(
                        models.len(),
                        locale,
                    )),
            );
        if has_available {
            header_row = header_row.child(with_tooltip(
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if any_benchmarking {
                            this.benchmark_cancel(cx);
                        } else {
                            this.benchmark_all_watchdog_models(cx);
                        }
                    }))
                    .child(if any_benchmarking {
                        spinning_loader("setting-speed-all-spin", theme.accent)
                            .into_any_element()
                    } else {
                        zap_icon(theme.accent).into_any_element()
                    }),
                crate::i18n::pages::test_all_models_tooltip(locale),
            ));
        }
        card = card.child(header_row);
    }
    card = card.child(list);
    if models.is_empty() {
        card = card.child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::cloud_virtual_models_empty(locale)),
        );
    }

    card
}

fn cli_settings_section(app: &Ruum, cx: &Context<'_, Ruum>) -> AnyElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
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
    let cli_btn_label = if cli_installed {
        crate::i18n::pages::cli_uninstall_btn(locale)
    } else {
        crate::i18n::pages::cli_install_btn(locale)
    };
    dialog_popup(cx)
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
                        action_button_with_spinner(
                            "setting-cli-btn",
                            "setting-cli-btn-spin",
                            cli_btn_label,
                            app.cli_updating,
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
        )
        .into_any_element()
}

fn settings_nav_tab(
    id: &'static str,
    category: SettingsCategory,
    icon_name: IconName,
    label: &'static str,
    active: bool,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .cursor_pointer()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::SM)
        .px(space::MD)
        .py(space::SM)
        .rounded(space::XS)
        .border_1()
        .border_color(if active {
            theme.accent.opacity(0.45)
        } else {
            theme.accent.opacity(0.0)
        })
        .bg(if active {
            theme.accent.opacity(0.16)
        } else {
            theme.accent.opacity(0.0)
        })
        .text_size(text::BODY)
        .text_color(if active {
            theme.bright
        } else {
            theme.secondary
        })
        .hover(|s| s.bg(theme.hover_fill()).text_color(theme.bright))
        .child(gpui_omarchy::icon(icon_name).size(icon::MD).text_color(
            if active {
                theme.accent
            } else {
                theme.secondary
            },
        ))
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_settings_category(category, cx);
        }))
}

/// Settings configuration page with Zed/VS Code style categorized sidebar.
pub fn settings_page(
    app: &Ruum,
    window: &mut Window,
    cx: &mut Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;

    let nav_sidebar = div()
        .id("settings-nav")
        .debug_selector(|| "settings-nav".into())
        .flex()
        .flex_col()
        .flex_shrink_0()
        .w(size::SETTINGS_NAV)
        .h_full()
        .border_r_1()
        .border_color(theme.divider())
        .p(space::MD)
        .gap(space::XS)
        .child(settings_nav_tab(
            "setting-tab-general",
            SettingsCategory::General,
            IconName::Settings,
            crate::i18n::pages::settings_tab_general(locale),
            app.settings_category == SettingsCategory::General,
            cx,
        ))
        .child(settings_nav_tab(
            "setting-tab-watchdog",
            SettingsCategory::Watchdog,
            IconName::Eye,
            crate::i18n::pages::settings_tab_watchdog(locale),
            app.settings_category == SettingsCategory::Watchdog,
            cx,
        ))
        .child(settings_nav_tab(
            "setting-tab-webhook",
            SettingsCategory::Webhook,
            IconName::Bell,
            crate::i18n::pages::settings_tab_webhook(locale),
            app.settings_category == SettingsCategory::Webhook,
            cx,
        ))
        .child(settings_nav_tab(
            "setting-tab-ai",
            SettingsCategory::Ai,
            IconName::Bot,
            crate::i18n::pages::settings_tab_ai(locale),
            app.settings_category == SettingsCategory::Ai,
            cx,
        ))
        .child(settings_nav_tab(
            "setting-tab-cli",
            SettingsCategory::Cli,
            IconName::SquareTerminal,
            crate::i18n::pages::settings_tab_cli(locale),
            app.settings_category == SettingsCategory::Cli,
            cx,
        ))
        .child(settings_nav_tab(
            "setting-tab-updates",
            SettingsCategory::Updates,
            IconName::Download,
            crate::i18n::pages::settings_tab_updates(locale),
            app.settings_category == SettingsCategory::Updates,
            cx,
        ));

    let section_body = match app.settings_category {
        SettingsCategory::General => general_settings_section(app, window, cx),
        SettingsCategory::Watchdog => {
            watchdog_settings_section(app, window, cx)
        }
        SettingsCategory::Webhook => webhook_settings_section(app, window, cx),
        SettingsCategory::Ai => ai_settings_section(app, window, cx),
        SettingsCategory::Cli => cli_settings_section(app, cx),
        SettingsCategory::Updates => updates_settings_section(app, window, cx),
    };

    let save_bar = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .px(space::XXL)
        .py(space::MD)
        .border_t_1()
        .border_color(theme.divider())
        .child(
            div().debug_selector(|| "setting-save-btn".into()).child(
                action_button_with_spinner(
                    "setting-save-btn",
                    "setting-save-btn-spin",
                    crate::i18n::pages::save_settings(locale),
                    app.watchdog_saving,
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
        .flex_row()
        .flex_1()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .child(nav_sidebar)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .px(space::XXL)
                        .pt(space::XL)
                        .pb(space::MD)
                        .child(
                            div().text_size(text::TITLE).child(
                                crate::i18n::pages::settings_title(locale),
                            ),
                        )
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::settings_subtitle(locale),
                        )),
                )
                .child(
                    div()
                        .id("settings-content")
                        .debug_selector(|| "settings-content".into())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(space::XXL)
                        .pb(space::LG)
                        .child(section_body),
                )
                .child(save_bar),
        )
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

    if modal.request.items.is_empty()
        && modal.request.docker_items.is_empty()
        && modal.request.docker_permission_hint.is_none()
    {
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

    if let Some(ref hint) = modal.request.docker_permission_hint {
        rows = rows.child(
            div()
                .id("confirm-docker-permission-denied")
                .debug_selector(|| "confirm-docker-permission-denied".into())
                .flex()
                .flex_col()
                .gap(space::XXS)
                .p(space::SM)
                .rounded(space::XS)
                .border_1()
                .border_color(theme.warning.opacity(0.45))
                .bg(theme.warning.opacity(0.08))
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.bright)
                        .child(crate::i18n::review::docker_cleanup_section(
                            locale,
                        )),
                )
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(
                            crate::i18n::review::docker_permission_denied_hint(
                                hint, locale,
                            ),
                        ),
                ),
        );
    }

    if !modal.request.docker_items.is_empty() {
        rows = rows.child(
            div()
                .pt(space::SM)
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::review::docker_cleanup_section(locale)),
        );
        for (group, group_key, group_label) in [
            (
                DockerGroupKind::BuildCache,
                "cache",
                crate::i18n::review::docker_group_build_cache(locale),
            ),
            (
                DockerGroupKind::Images,
                "images",
                crate::i18n::review::docker_group_unused_images(locale),
            ),
            (
                DockerGroupKind::Containers,
                "containers",
                crate::i18n::review::docker_group_stopped_containers(locale),
            ),
        ] {
            let group_indices: Vec<usize> = modal
                .request
                .docker_items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| {
                    let belongs = match group {
                        DockerGroupKind::BuildCache => {
                            matches!(item.target, DockerCleanTarget::BuildCache)
                        }
                        DockerGroupKind::Images => {
                            matches!(
                                item.target,
                                DockerCleanTarget::Image { .. }
                            )
                        }
                        DockerGroupKind::Containers => matches!(
                            item.target,
                            DockerCleanTarget::Container { .. }
                        ),
                    };
                    belongs.then_some(idx)
                })
                .collect();
            if group_indices.is_empty() {
                continue;
            }
            let checked_in_group = group_indices
                .iter()
                .filter(|&&idx| {
                    modal.is_docker_checked(
                        &modal.request.docker_items[idx].target,
                    )
                })
                .count();
            let group_state = if checked_in_group == 0 {
                CheckboxState::Unchecked
            } else if checked_in_group == group_indices.len() {
                CheckboxState::Checked
            } else {
                CheckboxState::Indeterminate
            };
            let entity_group = cx.entity().downgrade();
            rows = rows.child(
                div()
                    .debug_selector(move || {
                        format!("confirm-docker-group-{group_key}")
                    })
                    .pt(space::XS)
                    .child(
                        checkbox(
                            ElementId::Name(
                                format!("confirm-docker-group-{group_key}")
                                    .into(),
                            ),
                            group_label,
                            group_state,
                            cx,
                        )
                        .tab_stop(false)
                        .on_change(
                            move |_, _, _, cx| {
                                let _ = entity_group.update(cx, |this, cx| {
                                    this.modal.toggle_docker_group(group);
                                    cx.notify();
                                });
                            },
                        ),
                    ),
            );
            for idx in group_indices {
                let item = &modal.request.docker_items[idx];
                let is_checked = modal.is_docker_checked(&item.target);
                let check_state = if is_checked {
                    CheckboxState::Checked
                } else {
                    CheckboxState::Unchecked
                };
                let entity_docker = cx.entity().downgrade();
                let target_clone = item.target.clone();
                let mut row = div()
                    .id(ElementId::Name(
                        format!("confirm-docker-row-{idx}").into(),
                    ))
                    .debug_selector(move || format!("confirm-docker-row-{idx}"))
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
                                        format!("confirm-docker-check-{idx}")
                                    })
                                    .child(
                                        checkbox(
                                            ElementId::Name(
                                                format!(
                                                    "confirm-docker-check-{idx}"
                                                )
                                                .into(),
                                            ),
                                            item.label.clone(),
                                            check_state,
                                            cx,
                                        )
                                        .tab_stop(false)
                                        .on_change(move |_, _, _, cx| {
                                            let _ = entity_docker.update(
                                                cx,
                                                |this, cx| {
                                                    this.modal
                                                        .toggle_docker_target(
                                                            &target_clone,
                                                        );
                                                    cx.notify();
                                                },
                                            );
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(text::BODY)
                                    .text_color(theme.bright)
                                    .child(human_bytes(item.reclaimable_bytes)),
                            ),
                    );
                if let Some(ref detail) = item.detail {
                    row = row.child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.secondary)
                            .pl(space::LG)
                            .child(detail.clone()),
                    );
                }
                if let Some(ref reason) = item.reason {
                    row = row.child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.accent)
                            .pl(space::LG)
                            .child(reason.clone()),
                    );
                }
                rows = rows.child(row);
            }
        }
    }

    let summary_text = crate::i18n::pages::confirm_summary(
        modal.selected_count(),
        modal.total_count(),
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
        .on_action(cx.listener(
            |this, _: &crate::app_menu::CloseWindow, window, cx| {
                this.cancel(window, cx);
            },
        ))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => this.cancel(window, cx),
                "enter" => this.confirm(window, cx),
                _ => {}
            }
        }))
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .size_full()
        .p(space::XL)
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(theme.font)
        .text_size(text::BODY)
        .child(card)
}

/// Shared remote SSH connection confirmation card for overlay & standalone popup.
pub fn remote_connect_confirm_card<V: 'static>(
    modal: &RemoteConnectConfirmModal,
    locale: crate::i18n::Locale,
    on_approve: impl Fn(&mut V, &mut Window, &mut Context<'_, V>) + 'static,
    on_deny: impl Fn(&mut V, &mut Window, &mut Context<'_, V>) + 'static,
    cx: &Context<'_, V>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let req = &modal.request;
    let endpoint = format!("{}@{}:{}", req.username, req.host, req.port);
    let auth_label = match req.auth_mode.as_str() {
        "private_key" => crate::i18n::pages::auth_badge_private_key(locale),
        "password" => crate::i18n::pages::auth_badge_password(locale),
        other => other,
    };

    let detail_row = |label: &'static str, value: String| {
        div()
            .flex()
            .flex_row()
            .justify_between()
            .gap(space::MD)
            .child(div().text_color(theme.secondary).child(label))
            .child(div().text_color(theme.bright).child(value))
    };

    div()
        .id("remote-connect-confirm-popup")
        .debug_selector(|| "remote-connect-confirm-popup".into())
        .w_full()
        .max_w(size::HELP)
        .child(
            dialog_popup(cx)
                .w_full()
                .flex()
                .flex_col()
                .gap(space::MD)
        .child(dialog_title(
            crate::i18n::pages::remote_connect_title(locale),
            cx,
        ))
        .child(dialog_description(
            crate::i18n::pages::remote_connect_desc(&req.tool_name, locale),
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .p(space::SM)
                .rounded(space::XS)
                .border_1()
                .border_color(theme.divider())
                .bg(theme.normal_fill())
                .child(detail_row(
                    crate::i18n::pages::remote_connect_target_label(locale),
                    req.target_label.clone(),
                ))
                .child(detail_row(
                    crate::i18n::pages::remote_connect_endpoint_label(locale),
                    endpoint,
                ))
                .child(detail_row(
                    crate::i18n::pages::remote_connect_auth_label(locale),
                    auth_label.to_owned(),
                ))
                .child(detail_row(
                    crate::i18n::pages::remote_connect_tool_label(locale),
                    req.tool_name.clone(),
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(space::SM)
                .child(
                    div()
                        .debug_selector(|| "remote-connect-deny".into())
                        .child(
                            dialog_button(
                                "remote-connect-deny",
                                crate::i18n::pages::remote_connect_deny(locale),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    on_deny(this, window, cx);
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "remote-connect-approve".into())
                        .child(
                            dialog_button(
                                "remote-connect-approve",
                                crate::i18n::pages::remote_connect_approve(
                                    locale,
                                ),
                                ButtonVariant::Primary,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    on_approve(this, window, cx);
                                },
                            )),
                        ),
                ),
        ),
        )
}

/// Standalone interactive confirmation window content for MCP remote SSH
/// connection prompts.
pub fn remote_connect_confirm_view(
    modal: &RemoteConnectConfirmModal,
    focus: &FocusHandle,
    locale: crate::i18n::Locale,
    cx: &Context<'_, crate::RemoteConnectConfirmWindow>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let card = remote_connect_confirm_card(
        modal,
        locale,
        super::RemoteConnectConfirmWindow::approve,
        super::RemoteConnectConfirmWindow::deny,
        cx,
    );

    div()
        .id("remote-connect-confirm-window")
        .debug_selector(|| "remote-connect-confirm-window".into())
        .track_focus(focus)
        .key_context("RemoteConnectConfirm")
        .on_action(cx.listener(
            |this, _: &crate::app_menu::CloseWindow, window, cx| {
                this.deny(window, cx);
            },
        ))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => this.deny(window, cx),
                "enter" => this.approve(window, cx),
                _ => {}
            }
        }))
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(theme.font)
        .text_size(text::BODY)
        .child(
            div()
                .id("remote-connect-confirm-scroll")
                .flex()
                .flex_col()
                .items_center()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .overflow_y_scroll()
                .p(space::LG)
                .child(div().flex_1())
                .child(card)
                .child(div().flex_1()),
        )
}

/// Standalone compact progress window content for cancellable background cleanup.
pub fn cleanup_progress_view(
    snapshot: &crate::CleanupProgressSnapshot,
    focus: &FocusHandle,
    locale: crate::i18n::Locale,
    cx: &Context<'_, crate::CleanupProgressWindow>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let cancelling = snapshot.cancelling;
    let total = snapshot.total_items;
    let completed = snapshot.completed_items.min(total);
    let fraction = if total > 0 {
        (completed as f32 / total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let is_completed = matches!(
        snapshot.completion,
        Some(crate::CleanupProgressCompletion::Completed { .. })
    );
    let display_completed = if is_completed { total } else { completed };
    let display_fraction = if is_completed { 1.0 } else { fraction };
    let pct = (display_fraction * 100.0).round() as u64;
    let counter_text = format!("{display_completed}/{total} \u{00b7} {pct}%");
    let total_str =
        (snapshot.total_bytes > 0).then(|| human_bytes(snapshot.total_bytes));
    let reclaimed_text = crate::i18n::pages::cleanup_reclaimed_so_far(
        &human_bytes(snapshot.reclaimed_bytes),
        total_str.as_deref(),
        locale,
    );

    let (
        header_icon,
        heading_text,
        subtitle_text,
        subtitle_color,
        bar_fill,
        bottom_text,
        footer,
    ) = if let Some(ref completion) = snapshot.completion {
        let (icon_name, accent, cancelled, failed, subtitle, sub_color) =
            match completion {
                crate::CleanupProgressCompletion::Completed {
                    count,
                    bytes,
                } => (
                    IconName::CircleCheck,
                    theme.success,
                    false,
                    false,
                    crate::i18n::messages::cleaned_items_summary(
                        *count, *bytes, locale,
                    ),
                    theme.secondary,
                ),
                crate::CleanupProgressCompletion::Cancelled {
                    count,
                    bytes,
                } => (
                    IconName::TriangleAlert,
                    theme.warning,
                    true,
                    false,
                    crate::i18n::messages::cleanup_cancelled_summary(
                        *count, *bytes, locale,
                    ),
                    theme.warning,
                ),
                crate::CleanupProgressCompletion::Failed { message } => (
                    IconName::CircleX,
                    theme.danger,
                    false,
                    true,
                    message.clone(),
                    theme.danger,
                ),
            };
        let icon_el = div()
            .debug_selector(|| "cleanup-progress-done-icon".into())
            .flex()
            .items_center()
            .justify_center()
            .child(
                gpui_omarchy::icon(icon_name)
                    .size(icon::MD)
                    .flex_shrink_0()
                    .text_color(accent),
            )
            .into_any_element();
        let heading = crate::i18n::pages::cleanup_progress_done_heading(
            cancelled, failed, locale,
        );
        let auto_close = crate::i18n::pages::cleanup_progress_auto_closing(
            crate::CLEANUP_PROGRESS_LINGER.as_secs(),
            locale,
        );
        let footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(space::SM)
            .child(
                div()
                    .debug_selector(|| "cleanup-progress-close".into())
                    .child(
                    dialog_button(
                        "cleanup-progress-close",
                        crate::i18n::pages::cleanup_progress_close_button(
                            locale,
                        ),
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(|_, _, window, cx| {
                        crate::CleanupProgressWindow::continue_in_background(
                            window, cx,
                        );
                    })),
                ),
            )
            .into_any_element();
        (
            icon_el, heading, subtitle, sub_color, accent, auto_close, footer,
        )
    } else {
        let spin_color = if cancelling {
            theme.warning
        } else {
            theme.accent
        };
        let fill = if cancelling {
            theme.warning
        } else {
            theme.danger
        };
        let current_path_text =
            snapshot.current_item.clone().unwrap_or_else(|| {
                crate::i18n::pages::cleanup_progress_preparing(locale)
                    .to_owned()
            });
        let sub_color = if cancelling {
            theme.warning
        } else {
            theme.secondary
        };
        let icon_el = spinning_loader("cleanup-progress-spinner", spin_color)
            .into_any_element();
        let heading =
            crate::i18n::pages::cleanup_progress_heading(cancelling, locale);
        let subtitle =
            crate::i18n::pages::cleanup_progress_subtitle(cancelling, locale)
                .to_owned();
        let footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(space::SM)
            .child(
                div()
                    .debug_selector(|| "cleanup-progress-background".into())
                    .child(
                        dialog_button(
                            "cleanup-progress-background",
                            crate::i18n::pages::cleanup_continue_in_background(
                                locale,
                            ),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(|_, _, window, cx| {
                            crate::CleanupProgressWindow::continue_in_background(
                                window, cx,
                            );
                        })),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "cleanup-progress-cancel".into())
                    .child(
                        dialog_button(
                            "cleanup-progress-cancel",
                            crate::i18n::pages::cleanup_cancel_button(
                                cancelling, locale,
                            ),
                            if cancelling {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Danger
                            },
                            cx,
                        )
                        .disabled(cancelling)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_cleanup(cx);
                        })),
                    ),
            )
            .into_any_element();
        (
            icon_el,
            heading,
            subtitle,
            sub_color,
            fill,
            current_path_text,
            footer,
        )
    };

    let card = div()
        .id("cleanup-progress-popup")
        .debug_selector(|| "cleanup-progress-popup".into())
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
                .justify_between()
                .gap(space::MD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(space::SM)
                                .child(header_icon)
                                .child(
                                    div()
                                        .text_size(text::TITLE)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.bright)
                                        .child(heading_text),
                                ),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(text::CAPTION)
                                .text_color(subtitle_color)
                                .child(subtitle_text),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XS)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(space::SM)
                                .child(
                                    div()
                                        .id("cleanup-progress-counter")
                                        .debug_selector(|| {
                                            "cleanup-progress-counter".into()
                                        })
                                        .w(size::CLEANUP_COUNTER_LANE)
                                        .flex_shrink_0()
                                        .font_family(theme.mono_font.clone())
                                        .text_size(text::CAPTION)
                                        .text_color(theme.bright)
                                        .child(counter_text),
                                )
                                .child(
                                    div()
                                        .font_family(theme.mono_font.clone())
                                        .text_size(text::CAPTION)
                                        .text_color(theme.secondary)
                                        .child(reclaimed_text),
                                ),
                        )
                        .child(
                            div()
                                .id("cleanup-progress-bar")
                                .debug_selector(|| {
                                    "cleanup-progress-bar".into()
                                })
                                .w_full()
                                .h(size::METER)
                                .bg(theme.foreground.opacity(0.08))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(display_fraction))
                                        .bg(bar_fill),
                                ),
                        )
                        .child(
                            div()
                                .id("cleanup-progress-path")
                                .debug_selector(|| {
                                    "cleanup-progress-path".into()
                                })
                                .w_full()
                                .h(size::CLEANUP_PATH_LANE_H)
                                .min_w_0()
                                .truncate()
                                .font_family(theme.mono_font.clone())
                                .text_size(text::CAPTION)
                                .text_color(theme.secondary)
                                .child(bottom_text),
                        ),
                )
                .child(footer),
        );

    div()
        .id("cleanup-progress-window")
        .debug_selector(|| "cleanup-progress-window".into())
        .track_focus(focus)
        .key_context("CleanupProgress")
        .on_action(cx.listener(
            |_, _: &crate::app_menu::CloseWindow, window, cx| {
                crate::CleanupProgressWindow::continue_in_background(
                    window, cx,
                );
            },
        ))
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .size_full()
        .p(space::LG)
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(theme.font)
        .text_size(text::BODY)
        .child(card)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockerScope {
    Local,
    Remote,
}

/// Dedicated local Docker workspace (`Screen::Docker`) for inspecting usage
/// and cleaning build cache, unused images, and stopped containers.
pub fn docker_page(
    app: &Ruum,
    _window: &Window,
    cx: &Context<'_, Ruum>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let body = docker_status_body(
        div()
            .id("docker-page-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(space::XXL)
            .py(space::LG)
            .gap(space::LG),
        app,
        DockerScope::Local,
        &app.local_docker,
        &theme,
        cx,
    );

    div()
        .id("docker-page")
        .debug_selector(|| "docker-page".into())
        .flex()
        .flex_col()
        .flex_1()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .child(docker_header(app, &theme, cx))
        .child(body)
        .child(docker_footer(app, &theme, cx))
}

/// Centered modal overlay for inspecting remote SSH Docker usage and
/// generating a deterministic cleanup script from `Screen::Explore`.
pub fn remote_docker_modal(
    app: &Ruum,
    cx: &Context<'_, Ruum>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let source_text = remote_docker_source_label(app);
    let refresh_label = if app.docker_probing {
        crate::i18n::pages::docker_refreshing(locale)
    } else {
        crate::i18n::pages::docker_refresh_button(locale)
    };
    let selected_count = app.selected_review_docker_targets().len();
    let can_generate = matches!(app.docker, DockerProbeStatus::Available(_))
        && selected_count > 0;

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .gap(space::MD)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::MD)
                .child(dialog_title(
                    crate::i18n::pages::docker_page_title(locale),
                    cx,
                ))
                .child(
                    div()
                        .debug_selector(|| "docker-source-badge".into())
                        .px(space::SM)
                        .py(space::XXS)
                        .border_1()
                        .border_color(theme.divider())
                        .bg(theme.surface)
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(source_text),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::XS)
                .child(
                    div().debug_selector(|| "docker-refresh".into()).child(
                        action_button_with_spinner(
                            "docker-refresh",
                            "docker-refresh-spin",
                            refresh_label,
                            app.docker_probing,
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .disabled(app.docker_probing)
                        .on_click(cx.listener(
                            |this, _, window, cx| {
                                this.refresh_remote_docker(cx);
                                window.focus(&this.confirm_focus, cx);
                            },
                        )),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "remote-docker-close".into())
                        .child(
                            gpui_omarchy::icon_button(
                                "remote-docker-close",
                                IconName::Close,
                                crate::i18n::pages::docker_close_button(locale),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.close_remote_docker_modal(cx);
                                    this.apply_focus(window, cx);
                                },
                            )),
                        ),
                ),
        );

    let body = docker_status_body(
        div()
            .id("remote-docker-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(space::LG),
        app,
        DockerScope::Remote,
        &app.docker,
        &theme,
        cx,
    );

    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .gap(space::LG)
        .pt(space::MD)
        .border_t_1()
        .border_color(theme.divider())
        .child(crate::widgets::hint(
            "esc",
            crate::i18n::pages::docker_close_button(locale),
            cx,
        ))
        .child(
            div()
                .debug_selector(|| "docker-generate-script".into())
                .child(
                    button(
                        "docker-generate-script",
                        crate::i18n::review::generate_remote_script_button(
                            selected_count,
                            locale,
                        ),
                        ButtonVariant::Primary,
                        cx,
                    )
                    .disabled(!can_generate)
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.generate_remote_docker_script(cx);
                            this.apply_focus(window, cx);
                        },
                    )),
                ),
        );

    let popup = dialog_popup(cx)
        .w_full()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(header)
        .child(body)
        .child(footer);

    let card = div()
        .id("remote-docker-modal")
        .debug_selector(|| "remote-docker-modal".into())
        .track_focus(&app.confirm_focus)
        .flex()
        .flex_col()
        .w(size::DOCKER_MODAL)
        .max_w_full()
        .max_h_full()
        .child(popup);

    let close = cx.entity().downgrade();
    div()
        .id("remote-docker-backdrop")
        .absolute()
        .inset_0()
        .bg(theme.background.opacity(0.68))
        .child(crate::views::centred_popup(card, move |_, window, cx| {
            let _ = close.update(cx, |this, cx| {
                this.close_remote_docker_modal(cx);
                this.apply_focus(window, cx);
            });
        }))
}

fn docker_status_body(
    mut body: Stateful<Div>,
    app: &Ruum,
    scope: DockerScope,
    status: &DockerProbeStatus,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Stateful<Div> {
    let locale = app.locale;
    match status {
        DockerProbeStatus::NotInstalled => {
            body = body.child(docker_status_banner(
                "docker-not-installed",
                IconName::Info,
                crate::i18n::pages::docker_not_installed_title(locale),
                crate::i18n::pages::docker_not_installed_hint(locale),
                theme.secondary,
                theme,
            ));
        }
        DockerProbeStatus::DaemonStopped => {
            body = body.child(docker_status_banner(
                "docker-daemon-stopped",
                IconName::TriangleAlert,
                crate::i18n::pages::docker_daemon_stopped_title(locale),
                crate::i18n::pages::docker_daemon_stopped_hint(locale),
                theme.warning,
                theme,
            ));
        }
        DockerProbeStatus::PermissionDenied { socket_hint } => {
            let hint = crate::i18n::review::docker_permission_denied_hint(
                socket_hint,
                locale,
            );
            body = body.child(docker_status_banner(
                "docker-permission-denied",
                IconName::TriangleAlert,
                crate::i18n::pages::docker_permission_denied_title(locale),
                &hint,
                theme.warning,
                theme,
            ));
        }
        DockerProbeStatus::Available(summary) => {
            body = body
                .child(docker_overview_panel(summary, locale, theme))
                .child(docker_candidates_section(
                    app, scope, summary, theme, cx,
                ));
        }
    }
    body
}

fn remote_docker_source_label(app: &Ruum) -> String {
    let alias = app
        .active_ssh_profile_id
        .as_ref()
        .and_then(|id| app.ssh_profiles.iter().find(|p| &p.id == id))
        .map(|p| p.alias.as_str())
        .or(app.remote.as_deref())
        .unwrap_or("SSH");
    crate::i18n::pages::docker_source_ssh(alias, app.locale)
}

fn docker_header(
    app: &Ruum,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Div {
    let locale = app.locale;
    let source_text = crate::i18n::pages::docker_source_local(locale);
    let refresh_label = if app.local_docker_probing {
        crate::i18n::pages::docker_refreshing(locale)
    } else {
        crate::i18n::pages::docker_refresh_button(locale)
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .gap(space::MD)
        .px(space::XXL)
        .py(space::LG)
        .border_b_1()
        .border_color(theme.divider())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::MD)
                .child(
                    div()
                        .text_size(text::TITLE)
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.bright)
                        .child(crate::i18n::pages::docker_page_title(locale)),
                )
                .child(
                    div()
                        .debug_selector(|| "docker-source-badge".into())
                        .px(space::SM)
                        .py(space::XXS)
                        .border_1()
                        .border_color(theme.divider())
                        .bg(theme.surface)
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(source_text),
                ),
        )
        .child(
            div().debug_selector(|| "docker-refresh".into()).child(
                action_button_with_spinner(
                    "docker-refresh",
                    "docker-refresh-spin",
                    refresh_label,
                    app.local_docker_probing,
                    ButtonVariant::Secondary,
                    cx,
                )
                .disabled(app.local_docker_probing)
                .on_click(cx.listener(
                    |this, _, window, cx| {
                        this.refresh_local_docker_now(cx);
                        window.focus(&this.focus, cx);
                    },
                )),
            ),
        )
}

fn docker_status_banner(
    selector: &'static str,
    icon_name: IconName,
    title: &str,
    hint: &str,
    accent: Hsla,
    theme: &gpui_omarchy::Theme,
) -> Stateful<Div> {
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .flex()
        .flex_row()
        .items_start()
        .gap(space::MD)
        .p(space::LG)
        .border_1()
        .border_color(accent.opacity(0.45))
        .bg(accent.opacity(0.08))
        .child(
            div()
                .pt(space::XXS)
                .text_color(accent)
                .child(gpui_omarchy::icon(icon_name)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::XS)
                .min_w_0()
                .child(
                    div()
                        .text_size(text::TITLE)
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.bright)
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_size(text::BODY)
                        .text_color(theme.secondary)
                        .child(hint.to_owned()),
                ),
        )
}

fn docker_overview_panel(
    summary: &DockerUsageSummary,
    locale: crate::i18n::Locale,
    theme: &gpui_omarchy::Theme,
) -> Stateful<Div> {
    let highlight = palette::highlight(theme);
    let reclaimable = summary.safe_reclaimable_bytes();
    let total_bytes = summary
        .images
        .size_bytes
        .saturating_add(summary.containers.size_bytes)
        .saturating_add(summary.build_cache.size_bytes)
        .saturating_add(summary.local_volumes.size_bytes);

    let mut bar = div()
        .flex()
        .flex_row()
        .w_full()
        .h(space::SM)
        .bg(theme.foreground.opacity(0.08))
        .overflow_hidden();
    if total_bytes > 0 {
        let total_f = total_bytes as f32;
        for (bytes, color) in [
            (summary.images.size_bytes, theme.accent),
            (summary.containers.size_bytes, theme.warning),
            (summary.build_cache.size_bytes, highlight),
            (summary.local_volumes.size_bytes, theme.secondary),
        ] {
            if bytes > 0 {
                let ratio = (bytes as f32 / total_f).clamp(0.0, 1.0);
                bar = bar.child(div().h_full().w(relative(ratio)).bg(color));
            }
        }
    }

    let bc_reclaim = summary
        .build_cache_reclaimable_bytes
        .max(summary.build_cache.reclaimable_bytes);
    let shared_note = (summary.build_cache_shared_bytes > 0).then(|| {
        crate::i18n::pages::docker_card_shared_note(
            &human_bytes(summary.build_cache_shared_bytes),
            locale,
        )
    });

    div()
        .id("docker-overview")
        .debug_selector(|| "docker-overview".into())
        .flex()
        .flex_col()
        .gap(space::MD)
        .p(space::LG)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.surface)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::SM)
                .child(
                    div()
                        .text_size(text::TITLE)
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.bright)
                        .child(crate::i18n::pages::docker_overview_title(
                            locale,
                        )),
                )
                .child(
                    div().text_size(text::BODY).text_color(highlight).child(
                        crate::i18n::pages::docker_overview_reclaimable(
                            &human_bytes(reclaimable),
                            locale,
                        ),
                    ),
                ),
        )
        .child(bar)
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(space::MD)
                .child(docker_metric_card(
                    "docker-card-images",
                    crate::i18n::pages::docker_card_images(locale),
                    &human_bytes(summary.images.size_bytes),
                    &crate::i18n::pages::docker_card_meta(
                        summary.images.total_count,
                        summary.images.active_count,
                        &human_bytes(summary.images.reclaimable_bytes),
                        locale,
                    ),
                    theme.accent,
                    None,
                    None,
                    theme,
                ))
                .child(docker_metric_card(
                    "docker-card-containers",
                    crate::i18n::pages::docker_card_containers(locale),
                    &human_bytes(summary.containers.size_bytes),
                    &crate::i18n::pages::docker_card_meta(
                        summary.containers.total_count,
                        summary.containers.active_count,
                        &human_bytes(summary.containers.reclaimable_bytes),
                        locale,
                    ),
                    theme.warning,
                    None,
                    None,
                    theme,
                ))
                .child(docker_metric_card(
                    "docker-card-build-cache",
                    crate::i18n::pages::docker_card_build_cache(locale),
                    &human_bytes(summary.build_cache.size_bytes),
                    &crate::i18n::pages::docker_card_meta(
                        summary.build_cache.total_count,
                        summary.build_cache.active_count,
                        &human_bytes(bc_reclaim),
                        locale,
                    ),
                    highlight,
                    None,
                    shared_note.map(|text| ("docker-build-cache-shared", text)),
                    theme,
                ))
                .child(docker_metric_card(
                    "docker-card-volumes",
                    crate::i18n::pages::docker_card_local_volumes(locale),
                    &human_bytes(summary.local_volumes.size_bytes),
                    &crate::i18n::pages::docker_card_meta(
                        summary.local_volumes.total_count,
                        summary.local_volumes.active_count,
                        &human_bytes(summary.local_volumes.reclaimable_bytes),
                        locale,
                    ),
                    theme.secondary,
                    Some(crate::i18n::pages::docker_card_protected_badge(
                        locale,
                    )),
                    None,
                    theme,
                )),
        )
}

#[allow(
    clippy::too_many_arguments,
    reason = "metric card renders fixed four-slot summary fields"
)]
fn docker_metric_card(
    selector: &'static str,
    title: &str,
    size_text: &str,
    meta_text: &str,
    accent: Hsla,
    badge: Option<&str>,
    note: Option<(&'static str, String)>,
    theme: &gpui_omarchy::Theme,
) -> Stateful<Div> {
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .flex()
        .flex_col()
        .flex_1()
        .min_w(Rems(12.0))
        .gap(space::XS)
        .p(space::MD)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.inset)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::XS)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(space::XS)
                        .child(div().size(size::SWATCH).bg(accent))
                        .child(
                            div()
                                .text_size(text::BODY)
                                .text_color(theme.bright)
                                .child(title.to_owned()),
                        ),
                )
                .children(badge.map(|b| {
                    div()
                        .px(space::XS)
                        .border_1()
                        .border_color(theme.divider())
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(b.to_owned())
                })),
        )
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::BOLD)
                .text_color(theme.bright)
                .child(size_text.to_owned()),
        )
        .child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(meta_text.to_owned()),
        )
        .children(note.map(|(note_sel, text)| {
            div()
                .debug_selector(move || note_sel.into())
                .text_size(text::CAPTION)
                .text_color(theme.warning)
                .child(text)
        }))
}

fn docker_candidates_section(
    app: &Ruum,
    scope: DockerScope,
    summary: &DockerUsageSummary,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Stateful<Div> {
    let locale = app.locale;
    let (items, selected_count, selected_bytes) = match scope {
        DockerScope::Local => (
            app.local_docker_items().0,
            app.selected_local_docker_targets().len(),
            app.selected_local_docker_bytes(),
        ),
        DockerScope::Remote => (
            app.review_docker_items().0,
            app.selected_review_docker_targets().len(),
            app.selected_review_docker_bytes(),
        ),
    };
    let has_items = !items.is_empty();

    let mut panel = div()
        .id("docker-candidates")
        .debug_selector(|| "docker-candidates".into())
        .flex()
        .flex_col()
        .gap(space::SM)
        .p(space::LG)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.inset)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::SM)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::XXS)
                        .child(
                            div()
                                .text_size(text::TITLE)
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.bright)
                                .child(
                                    crate::i18n::review::docker_cleanup_section(
                                        locale,
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| {
                                    "docker-selected-summary".into()
                                })
                                .text_size(text::CAPTION)
                                .text_color(theme.secondary)
                                .child(format!(
                                    "{} \u{00b7} {}",
                                    crate::i18n::review::row_docker_items(
                                        locale
                                    ),
                                    crate::i18n::pages::docker_selected_summary(
                                        selected_count,
                                        &human_bytes(selected_bytes),
                                        locale,
                                    )
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(space::XS)
                        .child(
                            div()
                                .debug_selector(|| "docker-select-all".into())
                                .child(
                                    button(
                                        "docker-select-all",
                                        crate::i18n::review::docker_select_all(
                                            locale,
                                        ),
                                        ButtonVariant::Secondary,
                                        cx,
                                    )
                                    .disabled(!has_items)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            match scope {
                                                DockerScope::Local => this
                                                    .select_all_local_docker(),
                                                DockerScope::Remote => this
                                                    .select_all_review_docker(),
                                            }
                                            this.apply_focus(window, cx);
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| "docker-select-none".into())
                                .child(
                                    button(
                                        "docker-select-none",
                                        crate::i18n::review::docker_select_none(
                                            locale,
                                        ),
                                        ButtonVariant::Secondary,
                                        cx,
                                    )
                                    .disabled(!has_items)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            match scope {
                                                DockerScope::Local => this
                                                    .select_none_local_docker(),
                                                DockerScope::Remote => this
                                                    .select_none_review_docker(
                                                    ),
                                            }
                                            this.apply_focus(window, cx);
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                ),
        );

    if items.is_empty() {
        panel = panel.child(
            div()
                .p(space::MD)
                .border_1()
                .border_color(theme.divider())
                .bg(theme.surface)
                .text_size(text::BODY)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::docker_no_candidates_hint(
                    summary.detailed,
                    locale,
                )),
        );
    } else {
        for (group, group_key, group_label) in [
            (
                DockerGroupKind::BuildCache,
                "cache",
                crate::i18n::review::docker_group_build_cache(locale),
            ),
            (
                DockerGroupKind::Images,
                "images",
                crate::i18n::review::docker_group_unused_images(locale),
            ),
            (
                DockerGroupKind::Containers,
                "containers",
                crate::i18n::review::docker_group_stopped_containers(locale),
            ),
        ] {
            panel = docker_append_group_rows(
                panel,
                app,
                scope,
                &items,
                group,
                group_key,
                group_label,
                theme,
                cx,
            );
        }
    }

    panel.child(docker_volumes_notice_row(summary, locale, theme))
}

#[allow(
    clippy::too_many_arguments,
    reason = "group section helper shares parent styling and context"
)]
fn docker_append_group_rows(
    mut panel: Stateful<Div>,
    app: &Ruum,
    scope: DockerScope,
    items: &[ruum_core::confirm::DockerConfirmItem],
    group: DockerGroupKind,
    group_key: &'static str,
    group_label: &str,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Stateful<Div> {
    let group_items: Vec<_> = items
        .iter()
        .filter(|item| match group {
            DockerGroupKind::BuildCache => {
                matches!(item.target, DockerCleanTarget::BuildCache)
            }
            DockerGroupKind::Images => {
                matches!(item.target, DockerCleanTarget::Image { .. })
            }
            DockerGroupKind::Containers => {
                matches!(item.target, DockerCleanTarget::Container { .. })
            }
        })
        .collect();
    if group_items.is_empty() {
        return panel;
    }

    let checked_set = match scope {
        DockerScope::Local => &app.local_docker_checked,
        DockerScope::Remote => &app.review_docker_checked,
    };
    let checked_count = group_items
        .iter()
        .filter(|item| checked_set.contains(&item.target))
        .count();
    let group_state = if checked_count == 0 {
        CheckboxState::Unchecked
    } else if checked_count == group_items.len() {
        CheckboxState::Checked
    } else {
        CheckboxState::Indeterminate
    };
    let entity_group = cx.entity().downgrade();
    panel = panel.child(
        div()
            .debug_selector(move || format!("docker-group-{group_key}"))
            .pt(space::XS)
            .child(
                checkbox(
                    ElementId::Name(format!("docker-group-{group_key}").into()),
                    group_label.to_owned(),
                    group_state,
                    cx,
                )
                .tab_stop(false)
                .on_change(move |_, _, _, cx| {
                    let _ = entity_group.update(cx, |this, cx| {
                        match scope {
                            DockerScope::Local => {
                                this.toggle_local_docker_group(group);
                            }
                            DockerScope::Remote => {
                                this.toggle_review_docker_group(group);
                            }
                        }
                        cx.notify();
                    });
                }),
            ),
    );

    for (group_idx, item) in group_items.into_iter().enumerate() {
        panel = panel.child(docker_candidate_item_row(
            group_idx,
            scope,
            item,
            checked_set.contains(&item.target),
            theme,
            cx,
        ));
    }
    panel
}

fn docker_candidate_item_row(
    group_idx: usize,
    scope: DockerScope,
    item: &ruum_core::confirm::DockerConfirmItem,
    is_checked: bool,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Div {
    let selector = match &item.target {
        DockerCleanTarget::BuildCache => "docker-build-cache".to_owned(),
        DockerCleanTarget::Image { .. } => {
            format!("docker-image-{group_idx}")
        }
        DockerCleanTarget::Container { .. } => {
            format!("docker-container-{group_idx}")
        }
    };
    let check_state = if is_checked {
        CheckboxState::Checked
    } else {
        CheckboxState::Unchecked
    };
    let entity_item = cx.entity().downgrade();
    let target_clone = item.target.clone();
    let mut row = div()
        .flex()
        .flex_col()
        .gap(space::XXS)
        .px(space::SM)
        .py(space::XS)
        .border_1()
        .border_color(if is_checked {
            theme.accent.opacity(0.35)
        } else {
            theme.divider()
        })
        .bg(if is_checked {
            theme.accent.opacity(0.08)
        } else {
            theme.surface
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
                        .debug_selector({
                            let selector = selector.clone();
                            move || selector
                        })
                        .child(
                            checkbox(
                                ElementId::Name(selector.into()),
                                item.label.clone(),
                                check_state,
                                cx,
                            )
                            .tab_stop(false)
                            .on_change(
                                move |_, _, _, cx| {
                                    let _ =
                                        entity_item.update(cx, |this, cx| {
                                            match scope {
                                                DockerScope::Local => {
                                                    this.toggle_local_docker_target(
                                                        &target_clone,
                                                    );
                                                }
                                                DockerScope::Remote => {
                                                    this.toggle_review_docker_target(
                                                        &target_clone,
                                                    );
                                                }
                                            }
                                            cx.notify();
                                        });
                                },
                            ),
                        ),
                )
                .child(
                    div()
                        .text_size(text::BODY)
                        .text_color(theme.bright)
                        .child(human_bytes(item.reclaimable_bytes)),
                ),
        );
    if let Some(ref detail) = item.detail {
        row = row.child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .pl(space::LG)
                .child(detail.clone()),
        );
    }
    row
}

fn docker_volumes_notice_row(
    summary: &DockerUsageSummary,
    locale: crate::i18n::Locale,
    theme: &gpui_omarchy::Theme,
) -> Stateful<Div> {
    div()
        .id("docker-volumes-protected")
        .debug_selector(|| "docker-volumes-protected".into())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(space::SM)
        .mt(space::XS)
        .px(space::SM)
        .py(space::XS)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.surface)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::XXS)
                .child(
                    div()
                        .text_size(text::BODY)
                        .text_color(theme.foreground)
                        .child(
                            crate::i18n::pages::docker_volumes_notice_title(
                                locale,
                            ),
                        ),
                )
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(
                            crate::i18n::pages::docker_volumes_notice_detail(
                                summary.local_volumes.total_count,
                                &human_bytes(summary.local_volumes.size_bytes),
                                locale,
                            ),
                        ),
                ),
        )
        .child(
            div()
                .px(space::XS)
                .border_1()
                .border_color(theme.divider())
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::docker_card_protected_badge(locale)),
        )
}

fn docker_footer(
    app: &Ruum,
    theme: &gpui_omarchy::Theme,
    cx: &Context<'_, Ruum>,
) -> Div {
    let locale = app.locale;
    let selected_targets = app.selected_local_docker_targets();
    let selected_count = selected_targets.len();
    let can_execute =
        matches!(app.local_docker, DockerProbeStatus::Available(_))
            && selected_count > 0;

    let action_slot = div().debug_selector(|| "docker-commit".into()).child(
        button(
            "docker-commit",
            crate::i18n::pages::docker_commit_button(selected_count, locale),
            ButtonVariant::Primary,
            cx,
        )
        .disabled(!can_execute)
        .on_click(cx.listener(|this, _, window, cx| {
            this.commit_docker_cleanup(cx);
            this.apply_focus(window, cx);
        })),
    );

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .gap(space::LG)
        .px(space::XXL)
        .py(space::MD)
        .border_t_1()
        .border_color(theme.divider())
        .child(crate::widgets::hint(
            "esc",
            crate::i18n::review::back(locale),
            cx,
        ))
        .child(action_slot)
}

fn updates_settings_section(
    app: &Ruum,
    window: &mut Window,
    cx: &mut Context<'_, Ruum>,
) -> AnyElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;
    let is_checking = matches!(app.update_state, UpdateUiState::Checking);
    let busy = matches!(
        app.update_state,
        UpdateUiState::Checking
            | UpdateUiState::Downloading { .. }
            | UpdateUiState::Installing { .. }
    );

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0_i64, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let last_checked_text = if app.update_settings.last_checked_at_secs > 0 {
        let then = i64::try_from(app.update_settings.last_checked_at_secs)
            .unwrap_or(0);
        crate::i18n::common::ago(now_secs, then, locale)
    } else {
        crate::i18n::pages::updates_last_checked_never(locale).to_owned()
    };

    let (status_text, status_color, show_view_btn) = match &app.update_state {
        UpdateUiState::Idle => (String::new(), theme.secondary, false),
        UpdateUiState::Checking => (
            crate::i18n::pages::updates_checking(locale).to_owned(),
            theme.secondary,
            false,
        ),
        UpdateUiState::UpToDate { .. } => (
            crate::i18n::pages::updates_up_to_date(locale).to_owned(),
            theme.success,
            false,
        ),
        UpdateUiState::Available { info } => (
            crate::i18n::pages::rail_update_available(
                &info.latest_version,
                locale,
            ),
            theme.accent,
            true,
        ),
        UpdateUiState::Downloading { .. } => (
            crate::i18n::pages::rail_update_downloading(locale).to_owned(),
            theme.accent,
            true,
        ),
        UpdateUiState::ReadyToInstall { info, .. } => (
            crate::i18n::pages::rail_update_ready(&info.latest_version, locale),
            theme.success,
            true,
        ),
        UpdateUiState::Installing { .. } => (
            crate::i18n::pages::update_installing_status(locale).to_owned(),
            theme.accent,
            true,
        ),
        UpdateUiState::Error { message, .. } => (
            crate::i18n::pages::friendly_update_error(message, locale)
                .to_owned(),
            theme.danger,
            true,
        ),
    };

    let entity_auto = cx.entity().downgrade();
    let entity_chan = cx.entity().downgrade();
    let entity_cid = cx.entity().downgrade();
    let auto_check_on = app.update_settings.auto_check;
    let send_cid_on = app.update_settings.send_client_id;
    let selected_channel = match app.update_settings.channel {
        UpdateChannel::Stable => 0,
        UpdateChannel::Beta => 1,
    };

    let channel_group = div()
        .debug_selector(|| "setting-update-channel".into())
        .child(button_group(
            "setting-update-channel",
            vec![
                ChoiceItem::new(
                    "stable",
                    crate::i18n::pages::updates_channel_stable(locale),
                ),
                ChoiceItem::new(
                    "beta",
                    crate::i18n::pages::updates_channel_beta(locale),
                ),
            ],
            Some(selected_channel),
            move |index, _, cx| {
                let channel = if index == 1 {
                    UpdateChannel::Beta
                } else {
                    UpdateChannel::Stable
                };
                let _ = entity_chan.update(cx, |this, cx| {
                    this.set_update_channel(channel, cx);
                });
            },
            window,
            cx,
        ));

    div()
        .id("setting-updates-section")
        .debug_selector(|| "setting-updates-section".into())
        .w_full()
        .child(
            dialog_popup(cx)
                .w_full()
                .child(dialog_title(
                    crate::i18n::pages::updates_title(locale),
                    cx,
                ))
                .child(dialog_description(
                    crate::i18n::pages::updates_desc(locale),
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::SM)
                        .p(space::MD)
                        .rounded(space::XS)
                        .border_1()
                        .border_color(theme.divider())
                        .bg(theme.normal_fill())
                        .child(update_meta_row(
                            crate::i18n::pages::updates_current_version_label(
                                locale,
                            ),
                            format!("v{}", env!("CARGO_PKG_VERSION")),
                            true,
                            &theme,
                        ))
                        .child(update_meta_row(
                            crate::i18n::pages::updates_install_kind_label(
                                locale,
                            ),
                            crate::i18n::pages::install_kind_label(
                                app.update_install_kind,
                                locale,
                            )
                            .to_owned(),
                            false,
                            &theme,
                        ))
                        .child(update_meta_row(
                            crate::i18n::pages::updates_last_checked_label(
                                locale,
                            ),
                            last_checked_text,
                            false,
                            &theme,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(space::MD)
                                .pt(space::XS)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(text::CAPTION)
                                        .text_color(status_color)
                                        .child(status_text),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(space::SM)
                                        .when(show_view_btn, |row| {
                                            row.child(
                                                div()
                                                    .debug_selector(|| {
                                                        "setting-update-view-btn"
                                                            .into()
                                                    })
                                                    .child(
                                                        button(
                                                            "setting-update-view-btn",
                                                            crate::i18n::pages::update_view_btn(locale),
                                                            ButtonVariant::Secondary,
                                                            cx,
                                                        )
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.open_update_modal(cx);
                                                            },
                                                        )),
                                                    ),
                                            )
                                        })
                                        .child(
                                            div()
                                                .debug_selector(|| {
                                                    "setting-update-check-btn"
                                                        .into()
                                                })
                                                .child(
                                                    action_button_with_spinner(
                                                        "setting-update-check-btn",
                                                        "setting-update-check-spin",
                                                        crate::i18n::pages::updates_check_now(locale),
                                                        is_checking,
                                                        ButtonVariant::Primary,
                                                        cx,
                                                    )
                                                    .disabled(busy)
                                                    .on_click(cx.listener(
                                                        |this, _, _, cx| {
                                                            this.check_for_updates_now(cx);
                                                        },
                                                    )),
                                                ),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::MD)
                        .pt(space::SM)
                        .child(
                            div()
                                .debug_selector(|| {
                                    "setting-update-auto-check".into()
                                })
                                .child(
                                    checkbox(
                                        "setting-update-auto-check",
                                        crate::i18n::pages::updates_auto_check(
                                            locale,
                                        ),
                                        if auto_check_on {
                                            CheckboxState::Checked
                                        } else {
                                            CheckboxState::Unchecked
                                        },
                                        cx,
                                    )
                                    .tab_stop(false)
                                    .on_change(move |_, _, _, cx| {
                                        let _ = entity_auto.update(
                                            cx,
                                            |this, cx| {
                                                let next =
                                                    !this.update_settings.auto_check;
                                                this.set_update_auto_check(
                                                    next, cx,
                                                );
                                            },
                                        );
                                    }),
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
                                        .child(
                                            crate::i18n::pages::updates_channel_label(
                                                locale,
                                            ),
                                        ),
                                )
                                .child(channel_group),
                        )
                        .child(
                            div()
                                .debug_selector(|| {
                                    "setting-update-send-client-id".into()
                                })
                                .child(
                                    checkbox(
                                        "setting-update-send-client-id",
                                        crate::i18n::pages::updates_send_client_id(
                                            locale,
                                        ),
                                        if send_cid_on {
                                            CheckboxState::Checked
                                        } else {
                                            CheckboxState::Unchecked
                                        },
                                        cx,
                                    )
                                    .tab_stop(false)
                                    .on_change(move |_, _, _, cx| {
                                        let _ = entity_cid.update(
                                            cx,
                                            |this, cx| {
                                                let next = !this
                                                    .update_settings
                                                    .send_client_id;
                                                this.set_update_send_client_id(
                                                    next, cx,
                                                );
                                            },
                                        );
                                    }),
                                ),
                        ),
                ),
        )
        .into_any_element()
}

fn update_meta_row(
    label: &'static str,
    value: String,
    mono: bool,
    theme: &gpui_omarchy::Theme,
) -> Div {
    let mut val_el = div().text_color(theme.bright).child(value);
    if mono {
        val_el = val_el.font_family(theme.mono_font.clone());
    }
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(space::MD)
        .child(div().text_color(theme.secondary).child(label))
        .child(val_el)
}

/// Centered software update card modal (Layer 2).
pub fn update_modal(app: &Ruum, cx: &Context<'_, Ruum>) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let locale = app.locale;

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(space::MD)
        .child(dialog_title(
            crate::i18n::pages::update_modal_title(locale),
            cx,
        ))
        .child(
            div().debug_selector(|| "update-modal-close".into()).child(
                gpui_omarchy::icon_button(
                    "update-modal-close",
                    IconName::Close,
                    crate::i18n::pages::docker_close_button(locale),
                    ButtonVariant::Secondary,
                    cx,
                )
                .on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_update_modal(cx);
                        this.apply_focus(window, cx);
                    },
                )),
            ),
        );

    let mut body = div().flex().flex_col().gap(space::MD).min_w_0();

    if let Some(info) = app.update_state.info()
        && let Some(manifest) = &info.manifest
    {
        body = body
            .child(update_release_summary_card(info, manifest, locale, &theme));
        body =
            body.child(update_changelog_box(&manifest.notes, locale, &theme));
    }

    match &app.update_state {
        UpdateUiState::Idle => {
            body = body.child(dialog_description(
                format!(
                    "{} v{}",
                    crate::i18n::pages::updates_current_version_label(locale),
                    env!("CARGO_PKG_VERSION")
                ),
                cx,
            ));
        }
        UpdateUiState::Checking => {
            body =
                body.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(space::SM)
                        .child(spinning_loader(
                            "update-modal-check-spin",
                            theme.accent,
                        ))
                        .child(div().text_color(theme.secondary).child(
                            crate::i18n::pages::updates_checking(locale),
                        )),
                );
        }
        UpdateUiState::UpToDate { .. } => {
            body = body.child(
                div()
                    .text_color(theme.success)
                    .child(crate::i18n::pages::updates_up_to_date(locale)),
            );
        }
        UpdateUiState::Available { info } if !info.can_self_update => {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .p(space::SM)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.divider())
                    .bg(theme.normal_fill())
                    .child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.secondary)
                            .child(
                                crate::i18n::pages::update_manual_command_hint(
                                    locale,
                                ),
                            ),
                    )
                    .child(
                        div()
                            .font_family(theme.mono_font.clone())
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(info.upgrade_command.clone()),
                    ),
            );
        }
        UpdateUiState::Available { .. } => {}
        UpdateUiState::Downloading {
            downloaded, total, ..
        } => {
            let fraction = if *total > 0 {
                (*downloaded as f32 / *total as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let pct = if *total > 0 {
                ((*downloaded as f64 / *total as f64) * 100.0).round() as u64
            } else {
                0
            };
            let progress_label = if *total > 0 {
                format!(
                    "{} / {} ({pct}%)",
                    human_bytes(*downloaded),
                    human_bytes(*total)
                )
            } else {
                human_bytes(*downloaded)
            };
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .text_size(text::CAPTION)
                            .child(div().text_color(theme.secondary).child(
                                crate::i18n::pages::rail_update_downloading(
                                    locale,
                                ),
                            ))
                            .child(
                                div()
                                    .font_family(theme.mono_font.clone())
                                    .text_color(theme.bright)
                                    .child(progress_label),
                            ),
                    )
                    .child(
                        div()
                            .id("update-progress-bar")
                            .debug_selector(|| "update-progress-bar".into())
                            .w_full()
                            .h(size::METER)
                            .bg(theme.foreground.opacity(0.08))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .bg(theme.accent),
                            ),
                    ),
            );
        }
        UpdateUiState::ReadyToInstall { .. } => {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::XS)
                    .p(space::SM)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.success.opacity(0.45))
                    .bg(theme.success.opacity(0.08))
                    .child(
                        gpui_omarchy::icon(IconName::CircleCheck)
                            .size(icon::MD)
                            .flex_shrink_0()
                            .text_color(theme.success),
                    )
                    .child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(crate::i18n::pages::update_verified_badge(
                                locale,
                            )),
                    ),
            );
            let active_scan = app.scan.is_some() || app.remote_scan.is_some();
            let marks_count = app.marks.len();
            if active_scan || marks_count > 0 {
                let mut warn_box = div()
                    .id("update-restart-warning")
                    .debug_selector(|| "update-restart-warning".into())
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(space::SM)
                    .p(space::SM)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.warning.opacity(0.45))
                    .bg(theme.warning.opacity(0.08))
                    .child(
                        gpui_omarchy::icon(IconName::TriangleAlert)
                            .size(icon::MD)
                            .flex_shrink_0()
                            .text_color(theme.warning),
                    );
                let mut warn_lines =
                    div().flex().flex_col().gap(space::XXS).flex_1();
                if active_scan {
                    warn_lines = warn_lines.child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(
                                crate::i18n::pages::update_restart_warning_scan(
                                    locale,
                                ),
                            ),
                    );
                }
                if marks_count > 0 {
                    warn_lines = warn_lines.child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(
                                crate::i18n::pages::update_restart_warning_marks(
                                    marks_count,
                                    locale,
                                ),
                            ),
                    );
                }
                warn_box = warn_box.child(warn_lines);
                body = body.child(warn_box);
            }
        }
        UpdateUiState::Installing { .. } => {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(space::SM)
                    .child(spinning_loader("update-install-spin", theme.accent))
                    .child(div().text_color(theme.bright).child(
                        crate::i18n::pages::update_installing_status(locale),
                    )),
            );
        }
        UpdateUiState::Error { message, .. } => {
            let friendly =
                crate::i18n::pages::friendly_update_error(message, locale);
            body = body.child(
                div()
                    .id("update-error-box")
                    .debug_selector(|| "update-error-box".into())
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(space::SM)
                    .p(space::SM)
                    .rounded(space::XS)
                    .border_1()
                    .border_color(theme.danger.opacity(0.45))
                    .bg(theme.danger.opacity(0.08))
                    .child(
                        gpui_omarchy::icon(IconName::CircleX)
                            .size(icon::MD)
                            .flex_shrink_0()
                            .text_color(theme.danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(text::CAPTION)
                            .text_color(theme.bright)
                            .child(friendly),
                    ),
            );
        }
    }

    let footer = update_modal_footer(app, cx);

    let popup = dialog_popup(cx)
        .w_full()
        .flex()
        .flex_col()
        .gap(space::MD)
        .child(header)
        .child(body)
        .child(footer);

    let card = div()
        .id("update-modal")
        .debug_selector(|| "update-modal".into())
        .track_focus(&app.update_focus)
        .flex()
        .flex_col()
        .w(size::UPDATE_MODAL)
        .max_w_full()
        .max_h_full()
        .child(popup);

    let close = cx.entity().downgrade();
    div()
        .id("update-modal-backdrop")
        .absolute()
        .inset_0()
        .bg(theme.background.opacity(0.68))
        .child(crate::views::centred_popup(card, move |_, window, cx| {
            let _ = close.update(cx, |this, cx| {
                this.close_update_modal(cx);
                this.apply_focus(window, cx);
            });
        }))
}

fn update_release_summary_card(
    info: &UpdateCheckResult,
    manifest: &Manifest,
    locale: crate::i18n::Locale,
    theme: &gpui_omarchy::Theme,
) -> Div {
    let mut card = div()
        .debug_selector(|| "update-release-summary-card".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .p(space::SM)
        .rounded(space::XS)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.normal_fill())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::MD)
                .child(
                    div()
                        .font_family(theme.mono_font.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent)
                        .child(format!(
                            "v{} \u{2192} v{}",
                            info.current_version, info.latest_version
                        )),
                )
                .child(
                    div()
                        .text_size(text::CAPTION)
                        .text_color(theme.secondary)
                        .child(crate::i18n::pages::update_published_at(
                            &manifest.published_at,
                            locale,
                        )),
                ),
        );
    if let Some(asset) = &info.selected_asset {
        card = card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(space::MD)
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(
                    div()
                        .font_family(theme.mono_font.clone())
                        .child(asset.filename.clone()),
                )
                .child(div().child(human_bytes(asset.size))),
        );
    }
    card
}

fn update_changelog_box(
    notes: &str,
    locale: crate::i18n::Locale,
    theme: &gpui_omarchy::Theme,
) -> Stateful<Div> {
    let mut scroll = div()
        .id("update-changelog-scroll")
        .debug_selector(|| "update-changelog-scroll".into())
        .flex()
        .flex_col()
        .gap(space::XS)
        .max_h(size::CHANGELOG_MAX_H)
        .overflow_y_scroll()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .p(space::SM)
        .rounded(space::XS)
        .border_1()
        .border_color(theme.divider())
        .bg(theme.surface);

    if notes.trim().is_empty() {
        return scroll.child(
            div()
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(crate::i18n::pages::update_changelog_empty(locale)),
        );
    }

    for (idx, raw_line) in notes.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = line
            .strip_prefix("### ")
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("# "))
        {
            scroll = scroll.child(
                div()
                    .debug_selector(move || {
                        format!("update-changelog-heading-{idx}")
                    })
                    .pt(space::XXS)
                    .text_size(text::BODY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.accent)
                    .child(heading.trim().to_owned()),
            );
        } else if let Some(item) =
            line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
        {
            scroll = scroll.child(
                div()
                    .debug_selector(move || {
                        format!("update-changelog-item-{idx}")
                    })
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(space::XS)
                    .pl(space::SM)
                    .child(div().text_color(theme.accent).child("\u{2022}"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(render_inline_markdown(item.trim(), theme)),
                    ),
            );
        } else {
            scroll = scroll.child(render_inline_markdown(line, theme));
        }
    }

    scroll
}

fn render_inline_markdown(text: &str, theme: &gpui_omarchy::Theme) -> Div {
    if !text.contains('`') {
        return div()
            .text_size(text::BODY)
            .text_color(theme.foreground)
            .child(text.to_owned());
    }

    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(space::XXS);

    for (idx, seg) in text.split('`').enumerate() {
        if seg.is_empty() {
            continue;
        }
        if idx % 2 == 1 {
            row = row.child(
                div()
                    .font_family(theme.mono_font.clone())
                    .text_size(text::CAPTION)
                    .text_color(theme.bright)
                    .bg(theme.normal_fill())
                    .px(space::XXS)
                    .rounded(space::XXS)
                    .child(seg.to_owned()),
            );
        } else {
            for word in seg.split_whitespace() {
                row = row.child(
                    div()
                        .text_size(text::BODY)
                        .text_color(theme.foreground)
                        .child(word.to_owned()),
                );
            }
        }
    }

    row
}

fn update_modal_footer(app: &Ruum, cx: &Context<'_, Ruum>) -> Div {
    let locale = app.locale;
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(space::SM);

    match &app.update_state {
        UpdateUiState::Available { info } => {
            let ver = info.latest_version.clone();
            row = row.child(
                div().debug_selector(|| "update-skip-btn".into()).child(
                    dialog_button(
                        "update-skip-btn",
                        crate::i18n::pages::update_skip_version(&ver, locale),
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.skip_update_version(cx);
                            this.apply_focus(window, cx);
                        },
                    )),
                ),
            );
            row = row.child(
                div().debug_selector(|| "update-later-btn".into()).child(
                    dialog_button(
                        "update-later-btn",
                        crate::i18n::pages::update_later(locale),
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.dismiss_update_banner(cx);
                            this.close_update_modal(cx);
                            this.apply_focus(window, cx);
                        },
                    )),
                ),
            );
            if info.can_self_update {
                row = row.child(
                    div()
                        .debug_selector(|| "update-download-btn".into())
                        .child(
                            dialog_button(
                                "update-download-btn",
                                crate::i18n::pages::update_download_and_update(
                                    locale,
                                ),
                                ButtonVariant::Primary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.start_update_download(cx);
                                },
                            )),
                        ),
                );
            } else {
                row = row
                    .child(
                        div()
                            .debug_selector(|| "update-open-release-btn".into())
                            .child(
                                dialog_button(
                                    "update-open-release-btn",
                                    crate::i18n::pages::update_open_release(
                                        locale,
                                    ),
                                    ButtonVariant::Secondary,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.open_update_release_page(cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "update-copy-cmd-btn".into())
                            .child(
                                dialog_button(
                                    "update-copy-cmd-btn",
                                    crate::i18n::pages::update_copy_command(
                                        locale,
                                    ),
                                    ButtonVariant::Primary,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.copy_update_command(cx);
                                    }),
                                ),
                            ),
                    );
            }
        }
        UpdateUiState::Downloading { .. } => {
            row = row
                .child(
                    div().debug_selector(|| "update-cancel-btn".into()).child(
                        dialog_button(
                            "update-cancel-btn",
                            crate::i18n::pages::update_cancel(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cancel_update_download(cx);
                            },
                        )),
                    ),
                )
                .child(
                    div().debug_selector(|| "update-hide-btn".into()).child(
                        dialog_button(
                            "update-hide-btn",
                            crate::i18n::pages::update_hide(locale),
                            ButtonVariant::Primary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, window, cx| {
                                this.close_update_modal(cx);
                                this.apply_focus(window, cx);
                            },
                        )),
                    ),
                );
        }
        UpdateUiState::ReadyToInstall { .. } => {
            row = row
                .child(
                    div().debug_selector(|| "update-later-btn".into()).child(
                        dialog_button(
                            "update-later-btn",
                            crate::i18n::pages::update_later(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, window, cx| {
                                this.dismiss_update_banner(cx);
                                this.close_update_modal(cx);
                                this.apply_focus(window, cx);
                            },
                        )),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "update-restart-install-btn".into())
                        .child(
                            dialog_button(
                                "update-restart-install-btn",
                                crate::i18n::pages::update_restart_and_install(
                                    locale,
                                ),
                                ButtonVariant::Primary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.apply_ready_update(cx);
                                },
                            )),
                        ),
                );
        }
        UpdateUiState::Installing { .. } => {
            row = row.child(
                dialog_button(
                    "update-installing-btn",
                    crate::i18n::pages::update_restart_and_install(locale),
                    ButtonVariant::Primary,
                    cx,
                )
                .disabled(true),
            );
        }
        UpdateUiState::Error { info, .. } => {
            let can_retry_download =
                info.as_ref().is_some_and(|i| i.can_self_update);
            row = row
                .child(
                    div()
                        .debug_selector(|| "update-open-release-btn".into())
                        .child(
                            dialog_button(
                                "update-open-release-btn",
                                crate::i18n::pages::update_open_github_release(
                                    locale,
                                ),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.open_update_release_page(cx);
                                },
                            )),
                        ),
                )
                .child(
                    div().debug_selector(|| "update-retry-btn".into()).child(
                        dialog_button(
                            "update-retry-btn",
                            crate::i18n::pages::update_retry(locale),
                            ButtonVariant::Primary,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, _, cx| {
                                if can_retry_download {
                                    this.start_update_download(cx);
                                } else {
                                    this.check_for_updates_now(cx);
                                }
                            },
                        )),
                    ),
                );
        }
        UpdateUiState::Idle
        | UpdateUiState::Checking
        | UpdateUiState::UpToDate { .. } => {
            let checking = matches!(app.update_state, UpdateUiState::Checking);
            row = row
                .child(
                    div().debug_selector(|| "update-later-btn".into()).child(
                        dialog_button(
                            "update-later-btn",
                            crate::i18n::pages::docker_close_button(locale),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .on_click(cx.listener(
                            |this, _, window, cx| {
                                this.close_update_modal(cx);
                                this.apply_focus(window, cx);
                            },
                        )),
                    ),
                )
                .child(
                    div().debug_selector(|| "update-retry-btn".into()).child(
                        dialog_button(
                            "update-retry-btn",
                            crate::i18n::pages::updates_check_now(locale),
                            ButtonVariant::Primary,
                            cx,
                        )
                        .disabled(checking)
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.check_for_updates_now(cx);
                            },
                        )),
                    ),
                );
        }
    }

    row
}
