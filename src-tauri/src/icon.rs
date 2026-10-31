//! Workbench tray assets: 36px = 18pt at 2x on macOS.

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

#[cfg(target_os = "windows")]
const ASSETS: [&[u8]; 4] = [
    include_bytes!("../icons/tray/windows/disconnected.png"),
    include_bytes!("../icons/tray/windows/busy.png"),
    include_bytes!("../icons/tray/windows/connected.png"),
    include_bytes!("../icons/tray/windows/error.png"),
];

#[cfg(not(target_os = "windows"))]
const ASSETS: [&[u8]; 4] = [
    include_bytes!("../icons/tray/macos/disconnected.png"),
    include_bytes!("../icons/tray/macos/busy.png"),
    include_bytes!("../icons/tray/macos/connected.png"),
    include_bytes!("../icons/tray/macos/error.png"),
];

static IMAGES: LazyLock<[Image<'static>; 4]> = LazyLock::new(|| {
    ASSETS.map(|bytes| Image::from_bytes(bytes).expect("embedded Workbench tray PNG must be valid"))
});

pub fn image(state: TrayState) -> Image<'static> {
    let index = match state {
        TrayState::Idle => 0,
        TrayState::Busy => 1,
        TrayState::Connected => 2,
        TrayState::Problem => 3,
    };
    IMAGES[index].clone()
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
            let icon = image(*state);
            let width = if cfg!(target_os = "macos") { 44 } else { 36 };
            assert_eq!((icon.width(), icon.height()), (width, 36));
            assert_eq!(icon.rgba().len(), (width * 36 * 4) as usize);
            assert_eq!(state.is_template(), cfg!(target_os = "macos"));
            assert!(icon.rgba().chunks(4).any(|p| p[3] > 200));
            for other in &states[..i] {
                assert_ne!(icon.rgba(), image(*other).rgba());
            }
        }
        let alpha = |state| {
            image(state)
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
    fn windows_assets_have_dark_ink_and_a_light_edge_for_both_themes() {
        for bytes in [
            include_bytes!("../icons/tray/windows/disconnected.png").as_slice(),
            include_bytes!("../icons/tray/windows/busy.png").as_slice(),
            include_bytes!("../icons/tray/windows/connected.png").as_slice(),
            include_bytes!("../icons/tray/windows/error.png").as_slice(),
        ] {
            let icon = Image::from_bytes(bytes).unwrap();
            assert_eq!((icon.width(), icon.height()), (36, 36));
            assert!(icon.rgba().chunks(4).any(|p| p[0] < 32 && p[3] > 200));
            assert!(
                icon.rgba()
                    .chunks(4)
                    .any(|p| p[0] > 223 && p[3] > 200)
            );
            assert!(icon.rgba().chunks(4).any(|p| p[3] == 0));
        }
    }
}
