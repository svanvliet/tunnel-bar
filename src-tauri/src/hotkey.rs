//! Global shortcut that toggles the popover.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                crate::tray::toggle_window(app);
            }
        })
        .build()
}

/// Parses an accelerator such as "CmdOrCtrl+Alt+Shift+T".
pub fn parse(accel: &str) -> Result<Shortcut, String> {
    let accel = accel.trim();
    if accel.is_empty() {
        return Err("Enter a shortcut, e.g. CmdOrCtrl+Alt+Shift+T.".into());
    }
    let s: Shortcut = accel.parse().map_err(|e| format!("Invalid shortcut “{accel}”: {e}"))?;
    if s.mods.is_empty() {
        return Err(format!("Shortcut “{accel}” needs at least one modifier (Cmd, Ctrl, Alt or Shift)."));
    }
    Ok(s)
}

/// Replaces any registered shortcut with `accel` (None disables the hotkey).
pub fn apply(app: &AppHandle, accel: Option<&str>) -> Result<(), String> {
    let gs = app.global_shortcut();
    let new = accel.map(parse).transpose()?;
    gs.unregister_all().map_err(|e| e.to_string())?;
    if let Some(s) = new {
        gs.register(s).map_err(|e| format!("Couldn't register {}: {e}", accel.unwrap_or_default()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_accelerators() {
        assert!(parse(crate::model::DEFAULT_HOTKEY).is_ok());
        assert!(parse("Ctrl+Space").is_ok());
        assert!(parse("T").is_err(), "modifier required");
        assert!(parse("").is_err());
        assert!(parse("Cmd+Nope").is_err());
    }
}
