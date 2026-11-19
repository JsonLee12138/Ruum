//! Client-side window chrome for Linux desktops without server-side
//! decorations (for example Ubuntu GNOME Wayland).
//!
//! When the platform reports [`Decorations::Server`] (macOS, Windows, and
//! SSD-capable Linux compositors such as KDE or Hyprland), [`wrap_window_chrome`]
//! returns the wrapped content verbatim without adding borders, shadows, or a
//! duplicate titlebar.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, ClickEvent, CursorStyle, Decorations, Edges,
    InteractiveElement as _, IntoElement, MAX_BUTTONS_PER_SIDE, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, RenderOnce, ResizeEdge,
    SharedString, Size, StatefulInteractiveElement as _, Styled, Tiling,
    Window, WindowButton, WindowButtonLayout, WindowControls, div, point, px,
    transparent_black,
};
use gpui_omarchy::{ActiveTheme as _, IconName, Theme};

use crate::ui::{icon, size, space, text};

/// Outer shadow padding used for Linux CSD windows (`set_client_inset`).
pub const CSD_SHADOW_SIZE: Pixels = px(20.0);
const CSD_BORDER_SIZE: Pixels = px(1.0);
const CSD_RESIZE_HIT_SIZE: Pixels = px(4.0);

/// Fallback Linux window button layout (`left: empty`, `right: min, max, close`),
/// compiled unconditionally so macOS, Windows, and Linux all share and test the
/// same logic without `gpui`'s Linux-only `WindowButtonLayout::linux_default`.
#[must_use]
pub const fn default_linux_button_layout() -> WindowButtonLayout {
    WindowButtonLayout {
        left: [None; MAX_BUTTONS_PER_SIDE],
        right: [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            Some(WindowButton::Close),
        ],
    }
}

/// Return `true` when all four edges of a CSD window are tiled or maximized.
#[must_use]
pub const fn all_sides_tiled(tiling: Tiling) -> bool {
    tiling.top && tiling.bottom && tiling.left && tiling.right
}

/// Per-side visual frame inset from outer window bounds to the visible border.
#[must_use]
pub fn client_frame_insets(
    shadow_size: Pixels,
    tiling: Tiling,
) -> Edges<Pixels> {
    let mut insets = Edges::all(shadow_size);
    if tiling.top {
        insets.top = px(0.0);
    }
    if tiling.bottom {
        insets.bottom = px(0.0);
    }
    if tiling.left {
        insets.left = px(0.0);
    }
    if tiling.right {
        insets.right = px(0.0);
    }
    insets
}

/// Visual outer shadow padding; drops to zero when all sides are tiled while
/// `window.set_client_inset(CSD_SHADOW_SIZE)` remains constant to prevent
/// restore-size jumps.
#[must_use]
pub const fn visual_shadow_size(shadow_size: Pixels, tiling: Tiling) -> Pixels {
    if all_sides_tiled(tiling) {
        px(0.0)
    } else {
        shadow_size
    }
}

/// Hit-test the 8-direction resize band around the visible CSD frame border.
#[must_use]
pub fn resize_edge_for_point(
    pos: Point<Pixels>,
    size: Size<Pixels>,
    insets: Edges<Pixels>,
    tiling: Tiling,
    hit_size: Pixels,
) -> Option<ResizeEdge> {
    if all_sides_tiled(tiling) {
        return None;
    }
    let inner_left = insets.left;
    let inner_right = size.width - insets.right;
    let inner_top = insets.top;
    let inner_bottom = size.height - insets.bottom;

    let on_left = pos.x >= inner_left - hit_size
        && pos.x <= inner_left + hit_size
        && pos.y >= inner_top - hit_size
        && pos.y <= inner_bottom + hit_size;
    let on_right = pos.x >= inner_right - hit_size
        && pos.x <= inner_right + hit_size
        && pos.y >= inner_top - hit_size
        && pos.y <= inner_bottom + hit_size;
    let on_top = pos.y >= inner_top - hit_size
        && pos.y <= inner_top + hit_size
        && pos.x >= inner_left - hit_size
        && pos.x <= inner_right + hit_size;
    let on_bottom = pos.y >= inner_bottom - hit_size
        && pos.y <= inner_bottom + hit_size
        && pos.x >= inner_left - hit_size
        && pos.x <= inner_right + hit_size;

    if !tiling.top && !tiling.left && on_top && on_left {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top && !tiling.right && on_top && on_right {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom && !tiling.left && on_bottom && on_left {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom && !tiling.right && on_bottom && on_right {
        return Some(ResizeEdge::BottomRight);
    }
    if !tiling.top && on_top {
        return Some(ResizeEdge::Top);
    }
    if !tiling.bottom && on_bottom {
        return Some(ResizeEdge::Bottom);
    }
    if !tiling.left && on_left {
        return Some(ResizeEdge::Left);
    }
    if !tiling.right && on_right {
        return Some(ResizeEdge::Right);
    }
    None
}

/// Filter a titlebar button side against compositor-supported [`WindowControls`].
/// Close is always preserved.
#[must_use]
pub fn visible_buttons_for_side(
    slots: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    controls: WindowControls,
) -> Vec<WindowButton> {
    slots
        .iter()
        .flatten()
        .copied()
        .filter(|btn| match btn {
            WindowButton::Minimize => controls.minimize,
            WindowButton::Maximize => controls.maximize,
            WindowButton::Close => true,
        })
        .collect()
}

/// Icon corresponding to a window button and current maximization state.
#[must_use]
pub const fn window_button_icon(
    button: WindowButton,
    is_maximized: bool,
) -> IconName {
    match button {
        WindowButton::Minimize => IconName::WindowMinimize,
        WindowButton::Maximize => {
            if is_maximized {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            }
        }
        WindowButton::Close => IconName::WindowClose,
    }
}

/// Pure calculation of `(left, top, right, bottom)` chrome offsets around the
/// wrapped root view.
#[must_use]
pub fn window_content_margins_for(
    decorations: Decorations,
    shadow_size: Pixels,
    title_bar_px: f32,
) -> (f32, f32, f32, f32) {
    match decorations {
        Decorations::Server => (0.0, 0.0, 0.0, 0.0),
        Decorations::Client { tiling } => {
            let insets = client_frame_insets(shadow_size, tiling);
            let border = CSD_BORDER_SIZE.as_f32();
            let left =
                insets.left.as_f32() + if tiling.left { 0.0 } else { border };
            let right =
                insets.right.as_f32() + if tiling.right { 0.0 } else { border };
            let top = insets.top.as_f32()
                + if tiling.top { 0.0 } else { border }
                + title_bar_px;
            let bottom = insets.bottom.as_f32()
                + if tiling.bottom { 0.0 } else { border };
            (left, top, right, bottom)
        }
    }
}

/// Chrome offsets `(left, top, right, bottom)` in physical pixels for `window`.
#[must_use]
pub fn window_content_margins(window: &Window) -> (f32, f32, f32, f32) {
    let inset = window.client_inset().unwrap_or(CSD_SHADOW_SIZE);
    let title_bar_px = size::TITLE_BAR.to_pixels(window.rem_size()).as_f32();
    window_content_margins_for(window.window_decorations(), inset, title_bar_px)
}

/// Unconditionally subscribe `window` to appearance and button-layout changes
/// so CSD controls and theme styling refresh immediately even when `native_look`
/// is disabled.
pub fn observe_window_chrome(window: &Window) {
    window
        .observe_window_appearance(|window, _| window.refresh())
        .detach();
    window
        .observe_button_layout_changed(|window, _| window.refresh())
        .detach();
}

/// Snapshot of platform window state used by [`WindowChrome`].
#[derive(Clone, Copy, Debug)]
pub struct WindowChromeState {
    pub decorations: Decorations,
    pub controls: WindowControls,
    pub button_layout: WindowButtonLayout,
    pub is_maximized: bool,
    pub is_window_active: bool,
    pub window_size: Size<Pixels>,
}

impl WindowChromeState {
    #[must_use]
    pub fn from_window(window: &Window, cx: &App) -> Self {
        Self {
            decorations: window.window_decorations(),
            controls: window.window_controls(),
            button_layout: cx
                .button_layout()
                .unwrap_or_else(default_linux_button_layout),
            is_maximized: window.is_maximized(),
            is_window_active: window.is_window_active(),
            window_size: window.bounds().size,
        }
    }
}

type ResizeHandler = Rc<dyn Fn(ResizeEdge, &mut Window, &mut App)>;
type WindowActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type WindowMenuHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

/// Window-manager action hooks invoked by [`WindowChrome`].
#[derive(Clone)]
pub struct WindowChromeActions {
    pub start_resize: ResizeHandler,
    pub start_move: WindowActionHandler,
    pub zoom: WindowActionHandler,
    pub minimize: WindowActionHandler,
    pub show_window_menu: WindowMenuHandler,
    pub close: WindowActionHandler,
}

impl std::fmt::Debug for WindowChromeActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowChromeActions")
            .finish_non_exhaustive()
    }
}

impl WindowChromeActions {
    /// Default hooks wired to GPUI's native `Window` operations and `on_close`.
    pub fn new(on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self {
            start_resize: Rc::new(|edge, window, _| {
                window.start_window_resize(edge);
            }),
            start_move: Rc::new(|window, _| window.start_window_move()),
            zoom: Rc::new(|window, _| window.zoom_window()),
            minimize: Rc::new(|window, _| window.minimize_window()),
            show_window_menu: Rc::new(|pos, window, _| {
                window.show_window_menu(pos);
            }),
            close: Rc::new(on_close),
        }
    }
}

/// Element wrapper rendering Linux CSD shadows, resize edges, and titlebar
/// controls only when [`Decorations::Client`] is active.
#[derive(IntoElement)]
pub struct WindowChrome {
    title: SharedString,
    content: AnyElement,
    actions: WindowChromeActions,
    state_override: Option<WindowChromeState>,
}

impl std::fmt::Debug for WindowChrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowChrome")
            .field("title", &self.title)
            .field("state_override", &self.state_override)
            .finish_non_exhaustive()
    }
}

/// Wrap `content` in Linux CSD window chrome when [`Decorations::Client`] is
/// active; passes `content` through unchanged under [`Decorations::Server`].
pub fn wrap_window_chrome(
    title: impl Into<SharedString>,
    content: impl IntoElement,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> WindowChrome {
    WindowChrome {
        title: title.into(),
        content: content.into_any_element(),
        actions: WindowChromeActions::new(on_close),
        state_override: None,
    }
}

/// Wrap `content` with explicit [`WindowChromeState`] and [`WindowChromeActions`]
/// (used to verify CSD layout and interactions in headless tests).
#[cfg(test)]
pub fn wrap_window_chrome_with(
    title: impl Into<SharedString>,
    content: impl IntoElement,
    state: WindowChromeState,
    actions: WindowChromeActions,
) -> WindowChrome {
    WindowChrome {
        title: title.into(),
        content: content.into_any_element(),
        actions,
        state_override: Some(state),
    }
}

const fn cursor_style_for_resize_edge(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
            CursorStyle::ResizeUpLeftDownRight
        }
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
            CursorStyle::ResizeUpRightDownLeft
        }
    }
}

fn csd_shadows(is_window_active: bool) -> Vec<BoxShadow> {
    let opacity = if is_window_active { 1.0 } else { 0.7 };
    vec![
        BoxShadow {
            color: gpui_kit::Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.20 * opacity,
            },
            blur_radius: px(10.0),
            spread_radius: px(-1.0),
            offset: point(px(0.0), px(2.0)),
            inset: false,
        },
        BoxShadow {
            color: gpui_kit::Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.18 * opacity,
            },
            blur_radius: px(3.0),
            spread_radius: px(0.0),
            offset: point(px(0.0), px(1.0)),
            inset: false,
        },
    ]
}

fn resize_hit_zones(
    window_size: Size<Pixels>,
    shadow_size: Pixels,
    hit_size: Pixels,
    tiling: Tiling,
) -> Vec<AnyElement> {
    if all_sides_tiled(tiling) {
        return Vec::new();
    }
    let insets = client_frame_insets(shadow_size, tiling);
    let inner_left = insets.left;
    let inner_right = window_size.width - insets.right;
    let inner_top = insets.top;
    let inner_bottom = window_size.height - insets.bottom;
    let frame_origin = point(insets.left, insets.top);
    let band = hit_size + hit_size;
    let span_x = (inner_right - inner_left + band).max(px(0.0));
    let span_y = (inner_bottom - inner_top + band).max(px(0.0));

    let mut zones = Vec::new();
    let mut push_zone =
        |edge: ResizeEdge, origin: Point<Pixels>, zone_size: Size<Pixels>| {
            let local = origin - frame_origin;
            zones.push(
                div()
                    .absolute()
                    .left(local.x)
                    .top(local.y)
                    .w(zone_size.width)
                    .h(zone_size.height)
                    .cursor(cursor_style_for_resize_edge(edge))
                    .into_any_element(),
            );
        };

    if !tiling.top {
        push_zone(
            ResizeEdge::Top,
            point(inner_left - hit_size, inner_top - hit_size),
            Size::new(span_x, band),
        );
    }
    if !tiling.bottom {
        push_zone(
            ResizeEdge::Bottom,
            point(inner_left - hit_size, inner_bottom - hit_size),
            Size::new(span_x, band),
        );
    }
    if !tiling.left {
        push_zone(
            ResizeEdge::Left,
            point(inner_left - hit_size, inner_top - hit_size),
            Size::new(band, span_y),
        );
    }
    if !tiling.right {
        push_zone(
            ResizeEdge::Right,
            point(inner_right - hit_size, inner_top - hit_size),
            Size::new(band, span_y),
        );
    }

    if !tiling.top && !tiling.left {
        push_zone(
            ResizeEdge::TopLeft,
            point(inner_left - hit_size, inner_top - hit_size),
            Size::new(band, band),
        );
    }
    if !tiling.top && !tiling.right {
        push_zone(
            ResizeEdge::TopRight,
            point(inner_right - hit_size, inner_top - hit_size),
            Size::new(band, band),
        );
    }
    if !tiling.bottom && !tiling.left {
        push_zone(
            ResizeEdge::BottomLeft,
            point(inner_left - hit_size, inner_bottom - hit_size),
            Size::new(band, band),
        );
    }
    if !tiling.bottom && !tiling.right {
        push_zone(
            ResizeEdge::BottomRight,
            point(inner_right - hit_size, inner_bottom - hit_size),
            Size::new(band, band),
        );
    }
    zones
}

const fn csd_button_id(button: WindowButton) -> &'static str {
    match button {
        WindowButton::Minimize => "csd-btn-minimize",
        WindowButton::Maximize => "csd-btn-maximize",
        WindowButton::Close => "csd-btn-close",
    }
}

fn render_window_button(
    button: WindowButton,
    is_maximized: bool,
    is_window_active: bool,
    actions: &WindowChromeActions,
    theme: &Theme,
) -> AnyElement {
    let id = csd_button_id(button);
    let icon_name = window_button_icon(button, is_maximized);
    let fg = if is_window_active {
        theme.foreground
    } else {
        theme.secondary
    };
    let is_close = button == WindowButton::Close;
    let hover_bg = if is_close {
        theme.danger
    } else {
        theme.hover_fill()
    };
    let hover_fg = if is_close {
        theme.background
    } else {
        theme.bright
    };
    let active_bg = if is_close {
        theme.danger.opacity(0.85)
    } else {
        theme.pressed_fill()
    };
    let actions = actions.clone();

    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .items_center()
        .justify_center()
        .w(size::TITLE_BAR)
        .h_full()
        .flex_shrink_0()
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .active(move |s| s.bg(active_bg).text_color(hover_fg))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            match button {
                WindowButton::Minimize => (actions.minimize)(window, cx),
                WindowButton::Maximize => (actions.zoom)(window, cx),
                WindowButton::Close => (actions.close)(window, cx),
            }
        })
        .child(gpui_omarchy::icon(icon_name).size(icon::MD))
        .into_any_element()
}

fn render_button_group(
    group_id: &'static str,
    buttons: &[WindowButton],
    is_maximized: bool,
    is_window_active: bool,
    actions: &WindowChromeActions,
    theme: &Theme,
) -> AnyElement {
    div()
        .id(group_id)
        .debug_selector(move || group_id.into())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h_full()
        .children(buttons.iter().map(|&btn| {
            render_window_button(
                btn,
                is_maximized,
                is_window_active,
                actions,
                theme,
            )
        }))
        .into_any_element()
}

fn render_title_bar(
    title: SharedString,
    state: WindowChromeState,
    tiling: Tiling,
    actions: &WindowChromeActions,
    drag_flag: &Rc<Cell<bool>>,
    theme: &Theme,
) -> AnyElement {
    let left_buttons =
        visible_buttons_for_side(state.button_layout.left, state.controls);
    let right_buttons =
        visible_buttons_for_side(state.button_layout.right, state.controls);
    let insets = client_frame_insets(CSD_SHADOW_SIZE, tiling);

    let drag_down = Rc::clone(drag_flag);
    let drag_up = Rc::clone(drag_flag);
    let drag_out = Rc::clone(drag_flag);
    let drag_move = Rc::clone(drag_flag);
    let drag_click = Rc::clone(drag_flag);
    let move_action = Rc::clone(&actions.start_move);
    let zoom_action = Rc::clone(&actions.zoom);
    let menu_action = Rc::clone(&actions.show_window_menu);
    let window_size = state.window_size;
    let window_menu_enabled = state.controls.window_menu;

    let title_color = if state.is_window_active {
        theme.bright
    } else {
        theme.secondary
    };

    div()
        .id("csd-title-bar")
        .debug_selector(|| "csd-title-bar".into())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_shrink_0()
        .w_full()
        .h(size::TITLE_BAR)
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.divider())
        .on_mouse_down_out(move |_, _, _| {
            drag_out.set(false);
        })
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
            if resize_edge_for_point(
                ev.position,
                window_size,
                insets,
                tiling,
                CSD_RESIZE_HIT_SIZE,
            )
            .is_some()
            {
                drag_down.set(false);
                return;
            }
            drag_down.set(ev.click_count < 2);
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| {
            drag_up.set(false);
        })
        .on_mouse_move(move |_, window, cx| {
            if drag_move.get() {
                drag_move.set(false);
                move_action(window, cx);
            }
        })
        .on_click(move |ev: &ClickEvent, window, cx| {
            if ev.click_count() >= 2 {
                drag_click.set(false);
                zoom_action(window, cx);
            }
        })
        .on_mouse_down(
            MouseButton::Right,
            move |ev: &MouseDownEvent, window, cx| {
                if window_menu_enabled {
                    cx.stop_propagation();
                    menu_action(ev.position, window, cx);
                }
            },
        )
        .when(!left_buttons.is_empty(), |bar| {
            bar.child(render_button_group(
                "csd-controls-left",
                &left_buttons,
                state.is_maximized,
                state.is_window_active,
                actions,
                theme,
            ))
        })
        .child(
            div()
                .id("csd-title-text")
                .debug_selector(|| "csd-title-text".into())
                .flex_1()
                .min_w_0()
                .px(space::MD)
                .text_size(text::BODY)
                .font_family(theme.font.clone())
                .text_color(title_color)
                .truncate()
                .child(title),
        )
        .when(!right_buttons.is_empty(), |bar| {
            bar.child(render_button_group(
                "csd-controls-right",
                &right_buttons,
                state.is_maximized,
                state.is_window_active,
                actions,
                theme,
            ))
        })
        .into_any_element()
}

impl RenderOnce for WindowChrome {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self
            .state_override
            .unwrap_or_else(|| WindowChromeState::from_window(window, cx));
        let Decorations::Client { tiling } = state.decorations else {
            return self.content;
        };

        // Keep client_inset constant across tiled/maximized transitions so
        // restoring back to floating bounds does not double-count the inset.
        window.set_client_inset(CSD_SHADOW_SIZE);

        let visual_shadow = visual_shadow_size(CSD_SHADOW_SIZE, tiling);
        let insets = client_frame_insets(CSD_SHADOW_SIZE, tiling);
        let theme = cx.omarchy().clone();
        let drag_flag = window
            .use_keyed_state("csd-titlebar-drag", cx, |_, _| {
                Rc::new(Cell::new(false))
            })
            .read(cx)
            .clone();

        let resize_action = Rc::clone(&self.actions.start_resize);
        let window_size = state.window_size;

        let title_bar = render_title_bar(
            self.title,
            state,
            tiling,
            &self.actions,
            &drag_flag,
            &theme,
        );

        let frame = div()
            .id("csd-window-frame")
            .debug_selector(|| "csd-window-frame".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .cursor(CursorStyle::default())
            .border_color(theme.divider())
            .when(!tiling.top, |d| d.border_t(CSD_BORDER_SIZE))
            .when(!tiling.bottom, |d| d.border_b(CSD_BORDER_SIZE))
            .when(!tiling.left, |d| d.border_l(CSD_BORDER_SIZE))
            .when(!tiling.right, |d| d.border_r(CSD_BORDER_SIZE))
            .when(!tiling.is_tiled(), |d| {
                d.shadow(csd_shadows(state.is_window_active))
            })
            .on_mouse_move(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(title_bar)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .child(self.content),
                    ),
            );

        div()
            .id("csd-window-backdrop")
            .debug_selector(|| "csd-window-backdrop".into())
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(transparent_black())
            .when(!tiling.top, |d| d.pt(visual_shadow))
            .when(!tiling.bottom, |d| d.pb(visual_shadow))
            .when(!tiling.left, |d| d.pl(visual_shadow))
            .when(!tiling.right, |d| d.pr(visual_shadow))
            .on_mouse_down(
                MouseButton::Left,
                move |ev: &MouseDownEvent, window, cx| {
                    if all_sides_tiled(tiling) {
                        return;
                    }
                    if let Some(edge) = resize_edge_for_point(
                        ev.position,
                        window_size,
                        insets,
                        tiling,
                        CSD_RESIZE_HIT_SIZE,
                    ) {
                        resize_action(edge, window, cx);
                    }
                },
            )
            .child(frame)
            .when(!all_sides_tiled(tiling), |backdrop| {
                backdrop.child(div().absolute().size_full().children(
                    resize_hit_zones(
                        window_size,
                        CSD_SHADOW_SIZE,
                        CSD_RESIZE_HIT_SIZE,
                        tiling,
                    ),
                ))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_linux_layout_puts_min_max_close_on_right() {
        let layout = default_linux_button_layout();
        assert_eq!(layout.left, [None; MAX_BUTTONS_PER_SIDE]);
        assert_eq!(
            layout.right,
            [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ]
        );
    }

    #[test]
    fn client_frame_insets_and_visual_shadow_handle_tiling() {
        let floating = Tiling::default();
        assert_eq!(
            client_frame_insets(CSD_SHADOW_SIZE, floating),
            Edges::all(CSD_SHADOW_SIZE)
        );
        assert_eq!(
            visual_shadow_size(CSD_SHADOW_SIZE, floating),
            CSD_SHADOW_SIZE
        );

        let left_half = Tiling {
            top: true,
            bottom: true,
            left: true,
            right: false,
        };
        assert_eq!(
            client_frame_insets(CSD_SHADOW_SIZE, left_half),
            Edges {
                top: px(0.0),
                bottom: px(0.0),
                left: px(0.0),
                right: CSD_SHADOW_SIZE,
            }
        );
        assert_eq!(
            visual_shadow_size(CSD_SHADOW_SIZE, left_half),
            CSD_SHADOW_SIZE
        );

        let fully_tiled = Tiling::tiled();
        assert_eq!(
            client_frame_insets(CSD_SHADOW_SIZE, fully_tiled),
            Edges::all(px(0.0))
        );
        assert_eq!(visual_shadow_size(CSD_SHADOW_SIZE, fully_tiled), px(0.0));
        assert_eq!(CSD_SHADOW_SIZE, px(20.0));
    }

    #[test]
    fn resize_edge_for_point_detects_eight_directions_and_respects_tiling() {
        let win = Size::new(px(800.0), px(600.0));
        let hit = px(4.0);
        let floating = Tiling::default();
        let insets = client_frame_insets(CSD_SHADOW_SIZE, floating);
        // Inner visible rect: x in [20, 780], y in [20, 580].
        assert_eq!(
            resize_edge_for_point(
                point(px(20.0), px(20.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(780.0), px(20.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(20.0), px(580.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::BottomLeft)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(780.0), px(580.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::BottomRight)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(400.0), px(20.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(400.0), px(580.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(20.0), px(300.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(780.0), px(300.0)),
                win,
                insets,
                floating,
                hit,
            ),
            Some(ResizeEdge::Right)
        );
        // Interior point and outer-shadow point do not trigger resize.
        assert_eq!(
            resize_edge_for_point(
                point(px(400.0), px(300.0)),
                win,
                insets,
                floating,
                hit,
            ),
            None
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(5.0), px(300.0)),
                win,
                insets,
                floating,
                hit,
            ),
            None
        );

        // Tiled edges disable resize on those sides.
        let left_tiled = Tiling {
            top: true,
            bottom: false,
            left: true,
            right: false,
        };
        let left_insets = client_frame_insets(CSD_SHADOW_SIZE, left_tiled);
        assert_eq!(
            resize_edge_for_point(
                point(px(0.0), px(300.0)),
                win,
                left_insets,
                left_tiled,
                hit,
            ),
            None
        );
        assert_eq!(
            resize_edge_for_point(
                point(px(780.0), px(0.0)),
                win,
                left_insets,
                left_tiled,
                hit,
            ),
            Some(ResizeEdge::Right)
        );

        let all_tiled = Tiling::tiled();
        let all_insets = client_frame_insets(CSD_SHADOW_SIZE, all_tiled);
        assert_eq!(
            resize_edge_for_point(
                point(px(0.0), px(0.0)),
                win,
                all_insets,
                all_tiled,
                hit,
            ),
            None
        );
    }

    #[test]
    fn visible_buttons_for_side_respects_custom_layout_and_controls() {
        let custom = WindowButtonLayout {
            left: [
                Some(WindowButton::Close),
                Some(WindowButton::Minimize),
                None,
            ],
            right: [Some(WindowButton::Maximize), None, None],
        };
        let full = WindowControls::default();
        assert_eq!(
            visible_buttons_for_side(custom.left, full),
            vec![WindowButton::Close, WindowButton::Minimize]
        );
        assert_eq!(
            visible_buttons_for_side(custom.right, full),
            vec![WindowButton::Maximize]
        );

        let dialog_controls = WindowControls {
            fullscreen: false,
            maximize: false,
            minimize: false,
            window_menu: false,
        };
        assert_eq!(
            visible_buttons_for_side(custom.left, dialog_controls),
            vec![WindowButton::Close]
        );
        assert!(
            visible_buttons_for_side(custom.right, dialog_controls).is_empty()
        );

        let default_layout = default_linux_button_layout();
        assert_eq!(
            visible_buttons_for_side(default_layout.right, dialog_controls),
            vec![WindowButton::Close]
        );
    }

    #[test]
    fn window_button_icon_switches_restore_when_maximized() {
        assert_eq!(
            window_button_icon(WindowButton::Minimize, false),
            IconName::WindowMinimize
        );
        assert_eq!(
            window_button_icon(WindowButton::Maximize, false),
            IconName::WindowMaximize
        );
        assert_eq!(
            window_button_icon(WindowButton::Maximize, true),
            IconName::WindowRestore
        );
        assert_eq!(
            window_button_icon(WindowButton::Close, false),
            IconName::WindowClose
        );
    }

    #[test]
    fn window_content_margins_match_server_and_client_decorations() {
        assert_eq!(
            window_content_margins_for(
                Decorations::Server,
                CSD_SHADOW_SIZE,
                34.0,
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            window_content_margins_for(
                Decorations::Client {
                    tiling: Tiling::default(),
                },
                CSD_SHADOW_SIZE,
                34.0,
            ),
            (21.0, 55.0, 21.0, 21.0)
        );
        assert_eq!(
            window_content_margins_for(
                Decorations::Client {
                    tiling: Tiling::tiled(),
                },
                CSD_SHADOW_SIZE,
                34.0,
            ),
            (0.0, 34.0, 0.0, 0.0)
        );
    }
}
