//! Workbench tray assets: 44x36 templates on macOS (22x18pt at 2x); on Windows 32px solid glyphs in a
//! white set for dark taskbars and a black set for light ones.

use std::sync::LazyLock;
use tauri::image::Image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Busy,
    Connected,
    Problem,
}

impl TrayState {
    pub fn is_template(self) -> bool {
        cfg!(target_os = "macos")
    }
}

const WINDOWS_DARK: [&[u8]; 4] = [
    include_bytes!("../icons/tray/windows/dark/disconnected.png"),
    include_bytes!("../icons/tray/windows/dark/busy.png"),
    include_bytes!("../icons/tray/windows/dark/connected.png"),
    include_bytes!("../icons/tray/windows/dark/error.png"),
];

const WINDOWS_LIGHT: [&[u8]; 4] = [
    include_bytes!("../icons/tray/windows/light/disconnected.png"),
    include_bytes!("../icons/tray/windows/light/busy.png"),
    include_bytes!("../icons/tray/windows/light/connected.png"),
    include_bytes!("../icons/tray/windows/light/error.png"),
];

const MACOS: [&[u8]; 4] = [
    include_bytes!("../icons/tray/macos/disconnected.png"),
    include_bytes!("../icons/tray/macos/busy.png"),
    include_bytes!("../icons/tray/macos/connected.png"),
    include_bytes!("../icons/tray/macos/error.png"),
];

fn decode(assets: [&'static [u8]; 4]) -> [Image<'static>; 4] {
    assets.map(|bytes| Image::from_bytes(bytes).expect("embedded Workbench tray PNG must be valid"))
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
static MACOS_IMAGES: LazyLock<[Image<'static>; 4]> = LazyLock::new(|| decode(MACOS));
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static DARK_IMAGES: LazyLock<[Image<'static>; 4]> = LazyLock::new(|| decode(WINDOWS_DARK));
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static LIGHT_IMAGES: LazyLock<[Image<'static>; 4]> = LazyLock::new(|| decode(WINDOWS_LIGHT));

fn index(state: TrayState) -> usize {
    match state {
        TrayState::Idle => 0,
        TrayState::Busy => 1,
        TrayState::Connected => 2,
        TrayState::Problem => 3,
    }
}

/// The Windows asset for a taskbar theme (`light_taskbar` picks black ink).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn windows_image(state: TrayState, light_taskbar: bool) -> Image<'static> {
    let set = if light_taskbar { &LIGHT_IMAGES } else { &DARK_IMAGES };
    set[index(state)].clone()
}

/// `light_taskbar` is ignored on macOS, where the menu bar tints template images itself.
pub fn image(state: TrayState, light_taskbar: bool) -> Image<'static> {
    if cfg!(target_os = "windows") {
        windows_image(state, light_taskbar)
    } else {
        let _ = light_taskbar;
        MACOS_IMAGES[index(state)].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_states_are_distinct_and_correctly_sized() {
        let states = [
            TrayState::Idle,
            TrayState::Busy,
            TrayState::Connected,
            TrayState::Problem,
        ];
        for (i, state) in states.iter().enumerate() {
            let icon = image(*state, false);
            let (width, height) = if cfg!(target_os = "windows") { (32, 32) } else { (44, 36) };
            assert_eq!((icon.width(), icon.height()), (width, height));
            assert_eq!(icon.rgba().len(), (width * height * 4) as usize);
            assert_eq!(state.is_template(), cfg!(target_os = "macos"));
            assert!(icon.rgba().chunks(4).any(|p| p[3] > 200));
            for other in &states[..i] {
                assert_ne!(icon.rgba(), image(*other, false).rgba());
            }
        }
        let alpha = |state| {
            image(state, false)
                .rgba()
                .chunks(4)
                .map(|p| p[3] as u32)
                .sum::<u32>()
        };
        assert!(alpha(TrayState::Connected) > alpha(TrayState::Idle));
        assert!(alpha(TrayState::Busy) < alpha(TrayState::Idle));
    }

    #[test]
    fn macos_assets_are_black_and_alpha_including_attention() {
        for bytes in [
            include_bytes!("../icons/tray/macos/disconnected.png").as_slice(),
            include_bytes!("../icons/tray/macos/busy.png").as_slice(),
            include_bytes!("../icons/tray/macos/connected.png").as_slice(),
            include_bytes!("../icons/tray/macos/error.png").as_slice(),
        ] {
            let icon = Image::from_bytes(bytes).unwrap();
            assert!(
                icon.rgba()
                    .chunks(4)
                    .all(|p| p[3] == 0 || p[..3] == [0, 0, 0])
            );
        }
        let error = Image::from_bytes(include_bytes!("../icons/tray/macos/error.png")).unwrap();
        // The exclamation is an alpha knockout, so macOS can tint every state.
        assert_eq!((error.width(), error.height()), (44, 36));
        assert!(error.rgba()[(5 * 44 + 35) * 4 + 3] < 100);
    }

    #[test]
    fn windows_assets_are_solid_white_or_black_per_taskbar_theme() {
        let states = [TrayState::Idle, TrayState::Busy, TrayState::Connected, TrayState::Problem];
        for (light, ink) in [(false, 255u8), (true, 0u8)] {
            for (i, state) in states.iter().enumerate() {
                let icon = windows_image(*state, light);
                assert_eq!((icon.width(), icon.height()), (32, 32));
                // Single-colour ink (antialiased only in alpha), with transparent background.
                assert!(icon.rgba().chunks(4).all(|p| p[3] == 0 || p[..3] == [ink, ink, ink]));
                assert!(icon.rgba().chunks(4).any(|p| p[3] > 200));
                assert!(icon.rgba().chunks(4).any(|p| p[3] == 0));
                for other in &states[..i] {
                    assert_ne!(icon.rgba(), windows_image(*other, light).rgba());
                }
            }
        }
        // The exclamation is knocked out of the error badge (top-right), not drawn in the ink colour.
        let error = windows_image(TrayState::Problem, false);
        let badge_center = (8 * 32 + 25) * 4 + 3;
        assert!(error.rgba()[badge_center] < 100);
    }
}
