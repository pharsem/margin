use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, LogicalPosition, Manager, WebviewWindow, Wry};

pub struct Tray {
    menu: Menu<Wry>,
    show: CheckMenuItem<Wry>,
    collapse: CheckMenuItem<Wry>,
}

pub fn create(app: &AppHandle, visible: bool, collapsed: bool) -> tauri::Result<()> {
    let show = CheckMenuItem::with_id(app, "show", "Show panel", true, visible, None::<&str>)?;
    let collapse = CheckMenuItem::with_id(app, "collapse", "Collapse", true, collapsed, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check-inbox", "Check inbox now", true, None::<&str>)?;
    let install = MenuItem::with_id(app, "install-hooks", "Install Claude Code hooks", true, None::<&str>)?;
    let uninstall = MenuItem::with_id(app, "remove-hooks", "Remove Claude Code hooks", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&show, &collapse, &settings, &check, &separator, &install, &uninstall, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Margin")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => crate::toggle_panel(app),
            "collapse" => crate::toggle_collapsed(app),
            "settings" => crate::open_settings(app),
            "check-inbox" => crate::run_inbox_check(app),
            "install-hooks" => crate::install_hooks(app, true),
            "remove-hooks" => crate::install_hooks(app, false),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::toggle_panel(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(Tray { menu, show, collapse });
    Ok(())
}

/// Opens the tray menu in the panel. Its clicks reach the tray's `on_menu_event` handler.
pub fn popup(app: &AppHandle, window: &WebviewWindow, x: f64, y: f64) -> tauri::Result<()> {
    match app.try_state::<Tray>() {
        Some(tray) => window.popup_menu_at(&tray.menu, LogicalPosition::new(x, y)),
        None => Ok(()),
    }
}

/// Check items toggle themselves on click, so set them from the real state after each change.
pub fn sync(app: &AppHandle, visible: bool, collapsed: bool) {
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.show.set_checked(visible);
        let _ = tray.collapse.set_checked(collapsed);
    }
}
