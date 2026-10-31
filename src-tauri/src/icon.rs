//! Renders the tray icon at runtime (36x36 = 18pt @2x) so states don't need image assets.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Busy,
    Connected,
    Problem,
}

impl TrayState {
    /// macOS template images adapt to light/dark menu bars; the problem state needs color.
    pub fn is_template(self) -> bool {
        self != TrayState::Problem
    }
}

const SIZE: usize = 36;
const SS: usize = 4; // supersampling

fn coverage(state: TrayState, x: f32, y: f32) -> (f32, bool) {
    // Arch centered at (18, 24): outer radius 14, base line at y=29..31.
    let (cx, cy) = (18.0f32, 24.0f32);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    let thickness = if state == TrayState::Connected { 6.0 } else { 3.0 };
    let outer = 14.0;
    let in_arch = y <= cy && d <= outer && d >= outer - thickness;
    let legs = y > cy
        && y <= 30.0
        && ((x >= cx - outer && x <= cx - outer + thickness) || (x <= cx + outer && x >= cx + outer - thickness));
    let base = (29.0..=31.5).contains(&y) && (2.0..=34.0).contains(&x);
    let dot = match state {
        TrayState::Busy => ((x - cx).powi(2) + (y - 25.0).powi(2)).sqrt() <= 2.5,
        TrayState::Connected => ((x - cx).powi(2) + (y - 24.0).powi(2)).sqrt() <= 3.5,
        _ => false,
    };
    let badge = state == TrayState::Problem && ((x - 29.0).powi(2) + (y - 8.0).powi(2)).sqrt() <= 6.0;
    if badge {
        return (1.0, true);
    }
    ((in_arch || legs || base || dot) as u8 as f32, false)
}

pub fn render(state: TrayState) -> Vec<u8> {
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut fg, mut red) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let (c, is_badge) = coverage(state, x, y);
                    if is_badge {
                        red += c;
                    } else {
                        fg += c;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let (fg, red) = (fg / n, red / n);
            let i = (py * SIZE + px) * 4;
            if red > 0.0 {
                rgba[i..i + 4].copy_from_slice(&[0xE5, 0x3E, 0x3E, (red * 255.0) as u8]);
            } else if fg > 0.0 {
                // Template: black + alpha. Colored (problem): mid-gray, legible on light and dark bars.
                let c = if state.is_template() { 0 } else { 0x8E };
                rgba[i..i + 4].copy_from_slice(&[c, c, c, (fg * 255.0) as u8]);
            }
        }
    }
    rgba
}

pub fn size() -> u32 {
    SIZE as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_distinct_states() {
        let idle = render(TrayState::Idle);
        let conn = render(TrayState::Connected);
        let prob = render(TrayState::Problem);
        assert_eq!(idle.len(), 36 * 36 * 4);
        let alpha = |v: &Vec<u8>| v.chunks(4).map(|p| p[3] as u32).sum::<u32>();
        assert!(alpha(&conn) > alpha(&idle));
        assert!(prob.chunks(4).any(|p| p[0] == 0xE5 && p[3] > 200));
    }
}
