use gpui::{
    App, Background, Hsla, Menu, MenuItem, OsAction, Pixels, Point, SystemMenuType, Window,
    WindowBackgroundAppearance, linear_color_stop, linear_gradient, point, px, rgb,
};
use gpui_component::scroll::ScrollbarShow;
use gpui_component::{ActiveTheme, Theme, ThemeMode};

use crate::core::actions::*;
use crate::core::config::Config;
use crate::terminal::view::{
    ClearScrollback, CopyText, CutText, FindInTerminal, FindNext, FindPrevious, PasteText,
    RedoEdit, SelectAll, UndoEdit,
};
use crate::ui::i18n::{L10nKey, t};
use crate::ui::presets;
use crate::ui::presets::Fill;

pub(crate) fn traffic_light_position() -> Point<Pixels> {
    point(px(9.), px(13.))
}

pub(crate) fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("xtty").items([
            MenuItem::action(t(L10nKey::AppMenuAbout), About),
            MenuItem::action(t(L10nKey::AppMenuCheckForUpdates), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSettings), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t(L10nKey::AppMenuServices), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuHideApp), HideApp),
            MenuItem::action(t(L10nKey::AppMenuHideOthers), HideOthers),
            MenuItem::action(t(L10nKey::AppMenuShowAll), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuQuit), Quit),
        ]),
        Menu::new(t(L10nKey::AppMenuFile)).items([
            MenuItem::action(t(L10nKey::AppMenuNewTab), NewTab),
            MenuItem::action(t(L10nKey::AppMenuNewWorkspace), NewWorkspace),
            MenuItem::action(t(L10nKey::AppMenuNewWorktreeTab), NewWorktreeTab),
            MenuItem::separator(),
            // SSH is one of the reasons to pick tty7 and it had no entry in the
            // menu bar at all — the only routes were ⌘P and Settings, both of
            // which you have to already know about. Same labels as the palette,
            // so there is still one name per thing.
            MenuItem::action(t(L10nKey::CmdSshManageProfiles), OpenSshProfiles),
            MenuItem::action(t(L10nKey::CmdSshReconnect), RestartSshSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSplitRight), SplitRight),
            MenuItem::action(t(L10nKey::AppMenuSplitDown), SplitDown),
            MenuItem::action(t(L10nKey::AppMenuToggleBroadcastInput), ToggleBroadcastInput),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameTab), RenameTab),
            MenuItem::action(
                t(L10nKey::AppMenuCopyWorkingDirectory),
                CopyWorkingDirectory,
            ),
            MenuItem::action(t(L10nKey::AppMenuCopySessionId), CopyAgentSessionId),
            MenuItem::action(t(L10nKey::AppMenuForkSession), ForkAgentSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClosePaneTab), CloseActiveTab),
            MenuItem::action(t(L10nKey::AppMenuCloseOtherTabs), CloseOtherTabs),
            MenuItem::action(t(L10nKey::AppMenuCloseTabsRight), CloseTabsToTheRight),
            MenuItem::action(t(L10nKey::AppMenuReopenClosedTab), ReopenClosedTab),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameWorkspace), RenameWorkspace),
            MenuItem::action(t(L10nKey::AppMenuStopWorkspace), StopWorkspace),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuDeleteWorkspace), DeleteWorkspace),
        ]),
        Menu::new(t(L10nKey::AppMenuEdit)).items([
            MenuItem::os_action(t(L10nKey::AppMenuUndo), UndoEdit, OsAction::Undo),
            MenuItem::os_action(t(L10nKey::AppMenuRedo), RedoEdit, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t(L10nKey::AppMenuCut), CutText, OsAction::Cut),
            MenuItem::os_action(t(L10nKey::AppMenuCopy), CopyText, OsAction::Copy),
            MenuItem::os_action(t(L10nKey::AppMenuPaste), PasteText, OsAction::Paste),
            MenuItem::os_action(t(L10nKey::AppMenuSelectAll), SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFind), FindInTerminal),
            MenuItem::action(t(L10nKey::AppMenuFindNext), FindNext),
            MenuItem::action(t(L10nKey::AppMenuFindPrevious), FindPrevious),
        ]),
        Menu::new(t(L10nKey::AppMenuView)).items([
            MenuItem::action(t(L10nKey::AppMenuCommandPalette), TogglePalette),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuIncreaseFontSize), IncreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuDecreaseFontSize), DecreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuResetFontSize), ResetFontSize),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuLeftSidebar), ToggleLeftPanel),
            MenuItem::action(t(L10nKey::AppMenuRightPanel), ToggleRightPanel),
            MenuItem::action(t(L10nKey::AppMenuCodePanel), ToggleCodePanel),
            MenuItem::action(t(L10nKey::AppMenuTabBarPosition), ToggleTabSidebar),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::CmdSshRemoteFiles), ToggleSftp),
            MenuItem::action(t(L10nKey::CmdSshPortForwarding), ShowSshForwards),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFocusNextPane), FocusNextPane),
            MenuItem::action(t(L10nKey::AppMenuFocusPreviousPane), FocusPrevPane),
            // No "Zoom Pane" here. It is bound to ⇧⌘↵, and gpui's macOS menu
            // builder has no key equivalent for `enter`: it hands AppKit the
            // literal string "enter", which takes the "e" — so the item drew
            // itself as ⇧⌘E, which is Code Panel's chord, and pressing it
            // opened the code panel. A menu item that teaches the wrong key and
            // points at another command is worse than no menu item. Zoom Pane
            // keeps its place in ⌘P, in a pane's own menu (drawn by us, with
            // the right chord), and on the Keybindings page.
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClearScrollback), ClearScrollback),
            // No "Enter Full Screen" here: AppKit puts its own at the bottom of
            // any menu named View, so ours sat directly above it — the same
            // command twice, under two different shortcuts. ⌘↵ still works and
            // is listed on the Keybindings page.
        ]),
        Menu::new(t(L10nKey::AppMenuWindow)).items(window_menu_items(cx)),
        Menu::new(t(L10nKey::AppMenuHelp)).items([
            MenuItem::action(t(L10nKey::AppMenuDocumentation), OpenDocumentation),
            MenuItem::action(t(L10nKey::AppMenuKeyboardShortcuts), ShowKeyboardShortcuts),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuJoinDiscord), OpenDiscord),
            MenuItem::action(t(L10nKey::AppMenuReportIssue), ReportIssue),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRestartServer), RestartDaemon),
        ]),
    ]);
}

fn window_menu_items(cx: &App) -> Vec<MenuItem> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let order = crate::ui::windows::menu_order(cx);
    let store = crate::core::session::WorkspaceStore::all(cx);

    let slot_action = crate::ui::tab_strip::select_workspace_action;

    let mut items = vec![
        MenuItem::action(t(L10nKey::AppMenuMinimize), MinimizeWindow),
        MenuItem::action(t(L10nKey::AppMenuZoom), ZoomWindow),
        MenuItem::separator(),
    ];
    let workspace_start = items.len();
    let mut separated = false;
    for (i, (id, open)) in order.iter().enumerate() {
        let Some(workspace) = store.get(*id) else {
            continue;
        };
        let Some(action) = slot_action(i) else { break };
        if !open && !separated {
            separated = true;
            if items.len() > workspace_start {
                items.push(MenuItem::Separator);
            }
        }
        let name = crate::ui::machine_mirror::display_name(cx, workspace)
            .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string());
        let label = if *open {
            name
        } else {
            format!(
                "{}  —  {}",
                name,
                crate::ui::home::relative_time(now, workspace.last_active)
            )
        };
        items.push(MenuItem::Action {
            name: label.into(),
            action,
            os_action: None,
            checked: false,
            disabled: false,
        });
    }
    if items.len() == workspace_start {
        items.push(MenuItem::action(
            t(L10nKey::AppMenuNewWorkspace),
            NewWorkspace,
        ));
    }
    items
}

pub(crate) fn window_background(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, bg.opacity.unwrap_or(1.0))
}

/// The same preset fill with the alpha channel forced to 1 — used by the
/// settings overlay, which must stay opaque (workspace translucency must
/// never show through it) while still rendering the preset's gradient
/// design instead of collapsing to a flat solid color.
pub(crate) fn window_background_opaque(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, 1.0)
}

/// The workspace's own fill: the preset at whatever alpha the window opacity
/// and backdrop material asked for.
pub(crate) fn workspace_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background(bg),
        None => cx.theme().background.into(),
    }
}

/// The fill for a full-window overlay (settings, the opened file, the diff
/// view). Always opaque: the overlay covers the whole workspace, so window
/// translucency and the backdrop material must stop at it instead of showing
/// desktop through its text. Overlays pair this with
/// `app::overlay_surface_layers`, because their own opaque fill hides the
/// theme background image the workspace root paints beneath them.
pub(crate) fn overlay_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background_opaque(bg),
        None => cx.theme().background.alpha(1.0).into(),
    }
}

fn window_background_with_alpha(bg: &presets::ActiveBackground, alpha: f32) -> Background {
    let stop = |c: u32| -> Hsla {
        let mut h: Hsla = rgb(c).into();
        h.a = alpha;
        h
    };
    match bg.fill {
        Fill::Solid(c) => stop(c).into(),
        Fill::Vertical { top, bottom } => linear_gradient(
            180.,
            linear_color_stop(stop(top), 0.),
            linear_color_stop(stop(bottom), 1.),
        ),
        Fill::Horizontal { left, right } => linear_gradient(
            90.,
            linear_color_stop(stop(left), 0.),
            linear_color_stop(stop(right), 1.),
        ),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SystemAppearance {
    dark: bool,
}

impl gpui::Global for SystemAppearance {}

fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

pub(crate) fn refresh_system_appearance(cx: &mut App) {
    let dark = is_dark(cx.window_appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn note_system_appearance(window: &Window, cx: &mut App) {
    let dark = is_dark(window.appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn system_dark(cx: &App) -> bool {
    cx.try_global::<SystemAppearance>()
        .is_some_and(|appearance| appearance.dark)
}

pub(crate) fn effective_preset_id(cx: &App) -> String {
    let config = cx.global::<Config>();
    if !config.theme_follow_system {
        config.theme_preset.clone()
    } else if system_dark(cx) {
        config.theme_preset_dark.clone()
    } else {
        config.theme_preset_light.clone()
    }
}

/// Default background alpha used while a backdrop material is active and
/// neither the theme nor the config overrides the opacity — a fully opaque
/// fill would hide the material behind it.
pub(crate) const SYSTEM_MATERIAL_OPACITY: f32 = 0.82;

pub(crate) fn resolved_background_appearance(blur: bool) -> WindowBackgroundAppearance {
    if blur {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// macOS has no Windows-style backdrop materials.
pub(crate) fn material_active() -> bool {
    false
}

/// The window's background alpha when neither the config nor the theme sets
/// an explicit opacity. Always opaque on macOS unless theme/config overrides.
pub(crate) fn default_window_opacity() -> f32 {
    1.0
}

/// The fill for the workspace's large translucent surfaces (file sidebar,
/// right panel, SFTP panel). While a material is active and the window is
/// translucent, the surface paints on top of the already-alpha window
/// background, so its own alpha stacks (src-over) and the material would
/// show through far less than behind the terminal; a constant 0.15 keeps
/// the backdrop ratio at ~85% of the terminal's at every opacity setting.
/// `theme.sidebar` itself stays opaque — the settings theme picker paints
/// with it on top of the opaque settings overlay and must stay legible.
pub(crate) fn workspace_surface_color(cx: &App) -> Hsla {
    let base: Hsla = cx.theme().sidebar;
    let translucent = cx
        .try_global::<presets::ActiveBackground>()
        .and_then(|bg| bg.opacity)
        .is_some_and(|o| o < 1.0);
    if !translucent {
        return base;
    }
    let config = cx.global::<Config>();
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    let blur = config.window_blur.unwrap_or(theme.blur);
    if material_active() {
        base.alpha(0.15)
    } else {
        base
    }
}

pub(crate) fn background_appearance(cx: &App) -> WindowBackgroundAppearance {
    let config = cx.global::<Config>();
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    let blur = config.window_blur.unwrap_or(theme.blur);
    resolved_background_appearance(blur)
}

/// The appearance last handed to each live window, so `apply_theme` can skip
/// a `set_background_appearance` that would change nothing.
///
/// Worth the bookkeeping because the call is no longer cheap: with a DWM
/// material selected, gpui answers it by re-setting
/// `DWMWA_SYSTEMBACKDROP_TYPE` and forcing a non-client frame recalculation
/// (`SetWindowPos` with `SWP_FRAMECHANGED`). `apply_theme` runs on *every*
/// `Config` mutation in *every* window, so dragging the opacity slider —
/// which never changes the appearance — would otherwise recalc the frame
/// once per mouse-move sample and visibly stutter the drag.
#[derive(Default)]
struct AppliedAppearance(std::collections::HashMap<gpui::WindowId, WindowBackgroundAppearance>);

impl gpui::Global for AppliedAppearance {}

/// Records `appearance` for `window` and reports whether it differs from
/// what that window was last given. Entries for closed windows are dropped
/// first: window ids come from a slotmap and are reused, so a stale entry
/// could otherwise suppress the very first call for a brand-new window.
fn take_appearance_change(
    window: &Window,
    appearance: WindowBackgroundAppearance,
    cx: &mut App,
) -> bool {
    let id = window.window_handle().window_id();
    let live: std::collections::HashSet<gpui::WindowId> =
        cx.windows().iter().map(|w| w.window_id()).collect();
    let applied = cx.default_global::<AppliedAppearance>();
    applied.0.retain(|known, _| live.contains(known));
    applied.0.insert(id, appearance) != Some(appearance)
}

/// The interface face gpui-component started out with, read once before
/// anything overrode it.
///
/// [`Theme::change`] only rewrites `font_family` when the theme config names
/// one, and none of ours does — so a face written into the theme below stays
/// written for the life of the process. Keeping the stock value here is what
/// lets **Interface font family → Default** put the system face back on the
/// spot; without it the setting would save `None`, leave the window looking
/// exactly as it did, and only come true at the next launch.
struct StockUiFont(gpui::SharedString);

impl gpui::Global for StockUiFont {}

pub(crate) fn apply_theme(mut window: Option<&mut Window>, cx: &mut App) {
    let follow = cx.global::<Config>().theme_follow_system;
    if follow {
        sync_native_appearance(None);
        refresh_system_appearance(cx);
    }
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    // Cache the mode beside the machine tree: the daemon is a separate process
    // and reads it back when it spawns a Windows pane, where ConPTY drops an
    // OSC 11 background query before tty7's emulator can answer it. Derived
    // state, so it deliberately stays out of the user's `config.json`.
    crate::core::machine::note_appearance(theme.dark);
    let config = cx.global::<Config>();
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    let blur = config.window_blur.unwrap_or(theme.blur);
    // A material needs some translucency to be visible; without an explicit
    // opacity override it defaults to SYSTEM_MATERIAL_OPACITY instead of
    // 1.0. Derived from the *resolved* appearance so old builds where
    // Blur/Acrylic fall back to plain transparency stay opaque by default.
    let default_opacity = default_window_opacity();
    let opacity = config
        .window_opacity
        .or(theme.opacity)
        .unwrap_or(default_opacity);
    let opacity = (opacity < 1.0).then_some(opacity);
    if !follow {
        sync_native_appearance(Some(theme.dark));
    }
    let m = theme.neutrals();
    let surfaces = theme.surfaces();
    let sem = theme.semantics();
    let active = theme.active_palette(config.theme_legible_palette);

    let ui_font_family = config.ui_font_family.clone();

    if let Some(window) = window.as_deref_mut() {
        let appearance = resolved_background_appearance(blur);
        if take_appearance_change(window, appearance, cx) {
            window.set_background_appearance(appearance);
        }
    }

    Theme::change(mode, window.as_deref_mut(), cx);
    cx.set_global(active);
    cx.set_global(presets::ActiveBackground {
        fill: theme.background.clone(),
        opacity,
        image: theme.image.clone(),
    });
    cx.set_global(surfaces.clone());
    cx.set_global(presets::ActiveAccent(m.accent));
    // Same treatment as `Surfaces`: derived once here rather than recomputed
    // in `render`, because the graph reads it once per visible row per frame
    // and each entry costs a contrast bisection on three surfaces.
    cx.set_global(presets::ActiveLanes(theme.lanes()));

    if !cx.has_global::<StockUiFont>() {
        let stock = Theme::global(cx).font_family.clone();
        cx.set_global(StockUiFont(stock));
    }
    let stock = cx.global::<StockUiFont>().0.clone();

    let t = Theme::global_mut(cx);
    // Assigned in both directions, never only when set: this is the one place
    // that decides the chrome's face, so it has to say "back to stock" as
    // plainly as it says "use Inter".
    t.font_family = match ui_font_family {
        Some(family) if !family.trim().is_empty() => family.into(),
        _ => stock,
    };
    let mut base: Hsla = rgb(m.background).into();
    if let Some(o) = opacity {
        base.a = o;
    }
    t.background = base;
    t.foreground = rgb(m.foreground).into();
    t.border = rgb(m.border).into();
    t.secondary = rgb(m.secondary).into();
    // Ink, not fill. gpui-component resolves both of these from its *stock*
    // foreground, so a button's label and the detail panel's section headings
    // were the only text in the window not written in the preset's own colour.
    t.button_foreground = rgb(m.foreground).into();
    t.secondary_foreground = rgb(m.foreground).into();
    t.muted = rgb(m.muted).into();
    t.muted_foreground = rgb(m.muted_foreground).into();
    t.popover = rgb(m.popover).into();
    t.tokens.popover = Hsla::from(rgb(m.popover)).into();
    t.tokens.popover_foreground = Hsla::from(rgb(m.foreground)).into();
    // The field beside the token, not a duplicate of it: gpui-component reads
    // this one for every tooltip, dropdown menu and date picker it draws. Left
    // unset it keeps the stock near-white, so menus and tooltips ignored the
    // preset and came out brighter than the window they float over.
    t.popover_foreground = rgb(m.foreground).into();

    let accent_fill = rgb(surfaces.popover.cursor);
    let accent_text: Hsla = rgb(m.foreground).into();
    t.accent = accent_fill.into();
    t.accent_foreground = accent_text;
    t.tokens.accent = Hsla::from(accent_fill).into();
    t.tokens.accent_foreground = accent_text.into();

    let primary_base: Hsla = rgb(presets::mix(m.foreground, m.background, 0.20)).into();
    let primary_hover: Hsla = rgb(presets::mix(m.foreground, m.background, 0.30)).into();
    let primary_active: Hsla = rgb(presets::mix(m.foreground, m.background, 0.10)).into();
    t.primary = primary_base;
    t.primary_hover = primary_hover;
    t.primary_active = primary_active;
    t.tokens.primary = primary_base.into();
    t.tokens.primary_hover = primary_hover.into();
    t.tokens.primary_active = primary_active.into();
    t.tokens.button_primary = primary_base.into();
    t.tokens.button_primary_hover = primary_hover.into();
    t.tokens.button_primary_active = primary_active.into();

    let steps = |c: u32| {
        (
            Hsla::from(rgb(c)),
            Hsla::from(rgb(presets::mix(c, m.background, 0.15))),
            Hsla::from(rgb(presets::mix(c, m.foreground, 0.15))),
        )
    };

    let (ink, ink_hover, ink_active) = steps(sem.danger.ink);
    let (fill, fill_hover, fill_active) = steps(sem.danger.fill);
    let on_fill = Hsla::from(rgb(sem.danger.on_fill));
    t.danger = ink;
    t.danger_hover = ink_hover;
    t.danger_active = ink_active;
    t.danger_foreground = on_fill;
    t.tokens.danger = fill.into();
    t.tokens.danger_hover = fill_hover.into();
    t.tokens.danger_active = fill_active.into();
    t.tokens.danger_foreground = on_fill.into();
    t.tokens.button_danger = fill.into();
    t.tokens.button_danger_hover = fill_hover.into();
    t.tokens.button_danger_active = fill_active.into();
    t.tokens.button_danger_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.warning.ink);
    let (fill, fill_hover, fill_active) = steps(sem.warning.fill);
    let on_fill = Hsla::from(rgb(sem.warning.on_fill));
    t.warning = ink;
    t.warning_hover = ink_hover;
    t.warning_active = ink_active;
    t.warning_foreground = on_fill;
    t.tokens.warning = fill.into();
    t.tokens.warning_hover = fill_hover.into();
    t.tokens.warning_active = fill_active.into();
    t.tokens.warning_foreground = on_fill.into();
    t.tokens.button_warning = fill.into();
    t.tokens.button_warning_hover = fill_hover.into();
    t.tokens.button_warning_active = fill_active.into();
    t.tokens.button_warning_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.success.ink);
    let (fill, fill_hover, fill_active) = steps(sem.success.fill);
    let on_fill = Hsla::from(rgb(sem.success.on_fill));
    t.success = ink;
    t.success_hover = ink_hover;
    t.success_active = ink_active;
    t.success_foreground = on_fill;
    t.tokens.success = fill.into();
    t.tokens.success_hover = fill_hover.into();
    t.tokens.success_active = fill_active.into();
    t.tokens.success_foreground = on_fill.into();
    t.tokens.button_success = fill.into();
    t.tokens.button_success_hover = fill_hover.into();
    t.tokens.button_success_active = fill_active.into();
    t.tokens.button_success_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.info.ink);
    let (fill, fill_hover, fill_active) = steps(sem.info.fill);
    let on_fill = Hsla::from(rgb(sem.info.on_fill));
    t.info = ink;
    t.info_hover = ink_hover;
    t.info_active = ink_active;
    t.info_foreground = on_fill;
    t.tokens.info = fill.into();
    t.tokens.info_hover = fill_hover.into();
    t.tokens.info_active = fill_active.into();
    t.tokens.info_foreground = on_fill.into();
    t.tokens.button_info = fill.into();
    t.tokens.button_info_hover = fill_hover.into();
    t.tokens.button_info_active = fill_active.into();
    t.tokens.button_info_foreground = on_fill.into();

    // The command-line highlighter paints commands, flags, paths, strings and
    // operators with these, and the find bar flags a bad regex with `red`.
    // Unset they hold gpui-component's stock ramp, one ramp for every dark
    // preset and one for every light one — so the line you were typing kept a
    // palette the output right above it had already left behind.
    t.red = rgb(theme.ansi_ink(1)).into();
    t.green = rgb(theme.ansi_ink(2)).into();
    t.yellow = rgb(theme.ansi_ink(3)).into();
    t.blue = rgb(theme.ansi_ink(4)).into();
    t.magenta = rgb(theme.ansi_ink(5)).into();
    t.cyan = rgb(theme.ansi_ink(6)).into();

    t.link = rgb(sem.link.ink).into();
    t.link_hover = rgb(presets::mix(sem.link.ink, m.foreground, 0.25)).into();
    t.link_active = rgb(presets::mix(sem.link.ink, m.background, 0.20)).into();
    t.tokens.link = Hsla::from(rgb(sem.link.ink)).into();
    t.tokens.link_hover = Hsla::from(rgb(presets::mix(sem.link.ink, m.foreground, 0.25))).into();
    t.tokens.link_active = Hsla::from(rgb(presets::mix(sem.link.ink, m.background, 0.20))).into();

    let knob = if presets::is_lighter(m.background, m.foreground) {
        m.background
    } else {
        m.foreground
    };
    t.tokens.background = Hsla::from(rgb(m.background)).into();
    t.tokens.switch_thumb = Hsla::from(rgb(knob)).into();
    t.tokens.switch = Hsla::from(rgb(surfaces.window.selected)).into();

    // A filled slider track means the same thing an on switch does — "this is
    // the value you set" — so it gets the same colour, and its knob is the same
    // knob. Left alone the bar falls back to `tokens.primary`, the near-black we
    // give primary buttons, and the thumb to `primary_foreground`, which on a
    // dark theme is a black disc on a dark page. Side by side on one settings
    // page the two controls disagreed about what a set value looks like.
    //
    // Both were tried on `primary` — the neutral ramp the segmented controls
    // and the sidebar's selected page are drawn from — to settle a complaint
    // that the accent pills were the only saturated things on a screen of
    // greys. Reverted on sight: a dark-grey "on" against a light-grey "off" is
    // not a large enough step to read at a glance down a column of rows, and a
    // switch whose state you have to look twice at has lost the one job it has.
    // If the mismatch with the segmented controls is worth closing, it has to
    // close from the other end — by giving *them* some accent — not by taking
    // it away from here.
    t.tokens.slider_bar = Hsla::from(rgb(m.accent)).into();
    t.tokens.slider_thumb = Hsla::from(rgb(knob)).into();

    t.caret = rgb(m.caret).into();
    t.selection = rgb(m.selection).into();

    let scrollbar_thumb: Hsla = rgb(presets::mix(m.background, m.foreground, 0.18)).into();
    let scrollbar_thumb_hover: Hsla = rgb(presets::mix(m.background, m.foreground, 0.34)).into();
    t.scrollbar = gpui::transparent_black();
    t.scrollbar_thumb = scrollbar_thumb;
    t.scrollbar_thumb_hover = scrollbar_thumb_hover;
    t.tokens.scrollbar = gpui::transparent_black().into();
    t.tokens.scrollbar_thumb = scrollbar_thumb.into();
    t.tokens.scrollbar_thumb_hover = scrollbar_thumb_hover.into();

    // Not `should_auto_hide_scrollbars()`. That preference answers a question
    // about *legacy* scrollbars — the ones that take a gutter out of the layout
    // — and macOS says "don't hide them" for anyone with a mouse plugged in.
    // Ours are overlay bars painted on top of the content, so honouring it
    // parked an opaque bar over the switcher's tab column for the whole time
    // the panel was open, with nothing to fade it out. Every list in the app
    // gets the same bar, so it fades everywhere or nowhere.
    t.scrollbar_show = ScrollbarShow::Scrolling;

    t.radius = px(8.);

    // Flat controls, floating panels. `Theme::shadow` gates exactly one thing —
    // the `shadow_xs` an inline control (button, input, select trigger,
    // checkbox, radio, slider knob) paints under itself — and never the drop
    // shadow on a menu, tooltip or popover, which every one of those draws
    // unconditionally. Left on, every field and button in the window carried a
    // faint lift that nothing else here has: this chrome separates surfaces with
    // low-contrast fills and hairlines, so a control sitting a millimetre above
    // the panel was the one place claiming depth, and it read as a rendering
    // artefact rather than as a material. Panels that really do float keep
    // their shadow.
    t.shadow = false;

    let sidebar_bg = Hsla::from(rgb(m.sidebar));
    let sidebar_sel = rgb(surfaces.sidebar.selected);
    // `t.sidebar` stays the opaque theme token: the settings theme picker
    // paints with it on top of the (opaque) settings overlay, so diluting
    // it would wash out that panel. The workspace sidebar/right-panel
    // surfaces get their translucent variant at render time instead —
    // see `workspace_surface_color`.
    t.sidebar = sidebar_bg.into();
    t.tokens.sidebar = sidebar_bg.into();
    // The lighter tier: every `sidebar_border` site is a pane meeting another
    // pane, and both already carry their own fill. See `Neutrals`.
    t.sidebar_border = rgb(m.divider).into();
    t.sidebar_foreground = rgb(surfaces.sidebar.text_resting).into();
    t.sidebar_accent = sidebar_sel.into();
    t.tokens.sidebar_accent = Hsla::from(sidebar_sel).into();
    t.sidebar_accent_foreground = rgb(surfaces.sidebar.text_selected).into();

    t.list.active_highlight = true;
    t.list_active = rgb(surfaces.popover.cursor).into();
    t.list_active_border = rgb(surfaces.popover.cursor).into();
    t.list_hover = rgb(surfaces.popover.hover).into();

    t.input = rgb(surfaces.window.selected).into();
    t.tokens.input = Hsla::from(rgb(surfaces.window.selected)).into();

    let button_hover: Hsla = rgb(surfaces.window.hover).into();
    let button_active: Hsla = rgb(surfaces.window.selected).into();
    t.tokens.button_hover = button_hover.into();
    t.tokens.button_active = button_active.into();
    t.tokens.secondary_hover = button_hover.into();
    t.tokens.secondary_active = button_active.into();
    t.tokens.button_secondary_hover = button_hover.into();
    t.tokens.button_secondary_active = button_active.into();

    t.ring = rgb(m.accent).into();
    // Every splitter and panel edge lights up with this while you drag it, and
    // the drop target for a dragged-in file tints with it. Unset it is a fixed
    // blue from gpui-component's stock theme — the same blue under all nine
    // presets. It is an interaction highlight, so it belongs with the focus
    // ring and the on-switch.
    t.drag_border = rgb(m.accent).into();

    // The code editor's gutter fill and current-line band do not come from
    // `Theme` at all — gpui-component reads them off the *syntax* theme, which
    // is still its stock one. So the editor painted a #0a0a0a strip down the
    // line-number column and a #171717 band across the cursor's line, on top of
    // a preset background that is nowhere near either. That is the black edge.
    //
    // Cleared to transparent rather than repainted with the preset's own fill:
    // the code panel sits on the window background, which may be a gradient or
    // a wallpaper image, and any flat colour would show as a seam against it.
    // The current line and the invisibles become translucent ink for the same
    // reason — they read correctly on light and dark presets alike.
    //
    // A transparent gutter is only safe because our gpui-component fork clips
    // the editor's scrolling content to the right of the line-number column.
    // Upstream leans on the opaque gutter fill to hide horizontally scrolled
    // text, so on a stock build clearing this key lets the text run straight
    // across the line numbers.
    let ink: Hsla = rgb(m.foreground).into();
    let mut highlight = (*t.highlight_theme).clone();
    highlight.style.editor_background = Some(gpui::transparent_black());
    highlight.style.editor_gutter_background = Some(gpui::transparent_black());
    highlight.style.editor_active_line = Some(ink.opacity(0.06));
    highlight.style.editor_line_number = Some(rgb(m.muted_foreground).into());
    highlight.style.editor_active_line_number = Some(ink);
    highlight.style.editor_invisible = Some(ink.opacity(0.25));
    t.highlight_theme = std::sync::Arc::new(highlight);

    if let Some(window) = window.as_deref_mut() {
        window.set_traffic_light_position(traffic_light_position());
    }
}

/// Every switch in the window, built here so the on-state has one definition.
///
/// The accent is load-bearing, not decoration. Without `.color()` a switch's
/// on-state falls to `tokens.primary`, a dark neutral, and against the light
/// neutral of the off-state that is too small a step to read while scanning a
/// column of rows — you end up checking the thumb's position on each one. It
/// was tried that way and reverted; see the slider-bar note in `apply_theme`
/// for the other half of the pair.
pub(crate) fn switch(id: impl Into<gpui::ElementId>, cx: &App) -> gpui_component::switch::Switch {
    let accent = cx.global::<presets::ActiveAccent>().0;
    gpui_component::switch::Switch::new(id).color(Hsla::from(rgb(accent)))
}

pub(crate) fn apply_cursor_hide_mode(cx: &mut App) {
    let mode = if cx.global::<Config>().mouse_hide_while_typing {
        gpui::CursorHideMode::OnTypingAndAction
    } else {
        gpui::CursorHideMode::Never
    };
    cx.set_cursor_hide_mode(mode);
}

fn sync_native_appearance(dark: Option<bool>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let appearance = dark.and_then(|dark| {
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        NSAppearance::appearanceNamed(name)
    });
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn sync_native_appearance(_dark: Option<bool>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn effective_preset_follows_the_cached_system_appearance(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Config(crate::core::config::CoreConfig {
                theme_follow_system: true,
                theme_preset_light: "light-slot".into(),
                theme_preset_dark: "dark-slot".into(),
                ..Default::default()
            }));

            cx.set_global(SystemAppearance { dark: false });
            assert!(!system_dark(cx));
            assert_eq!(effective_preset_id(cx), "light-slot");

            cx.set_global(SystemAppearance { dark: true });
            assert!(system_dark(cx));
            assert_eq!(effective_preset_id(cx), "dark-slot");

            cx.global_mut::<Config>().theme_follow_system = false;
            assert_eq!(effective_preset_id(cx), Config::default().theme_preset);
        });
    }
}
