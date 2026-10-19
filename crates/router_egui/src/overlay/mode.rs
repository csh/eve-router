//! The display mode of a game client, from the numbers that Windows gives for its window.
//!
//! A measurement on a real client (title `EVE - smrkn`, one 3440x1440 monitor) in its borderless
//! mode gave: a window of the size of the monitor at its corner, the style `0x940a0000` (popup, no
//! caption, no thick frame), and no topmost bit. The windowed mode is not measured yet. It must
//! have a caption, as every normal window has.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{WS_BORDER, WS_DLGFRAME, WS_POPUP};

/// The display mode of a client window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// A window with no frame that covers the monitor. The overlay shows above it.
    Borderless,
    /// A window with a title bar. The overlay shows above it.
    Windowed,
    /// Any other shape. The window can be a true fullscreen window, and the overlay may stay
    /// hidden behind it.
    Other,
}

/// The numbers of one client window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub window: RECT,
    /// The whole area of the monitor that holds most of the window.
    pub monitor: RECT,
    /// The `GWL_STYLE` bits of the window.
    pub style: u32,
}

impl Geometry {
    pub fn mode(&self) -> DisplayMode {
        // WS_CAPTION is WS_BORDER and WS_DLGFRAME together. Test both bits.
        let caption = WS_BORDER.0 | WS_DLGFRAME.0;
        if self.style & caption == caption {
            DisplayMode::Windowed
        } else if self.style & WS_POPUP.0 != 0 && covers(&self.window, &self.monitor) {
            DisplayMode::Borderless
        } else {
            DisplayMode::Other
        }
    }
}

/// True when `outer` holds all of `inner`.
fn covers(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left && outer.top <= inner.top && outer.right >= inner.right && outer.bottom >= inner.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    const MONITOR: RECT = rect(0, 0, 3440, 1440);

    /// The values that the measurement gave.
    #[test]
    fn the_measured_borderless_client_is_borderless() {
        let g = Geometry { window: rect(0, 0, 3440, 1440), monitor: MONITOR, style: 0x940a_0000 };
        assert_eq!(g.mode(), DisplayMode::Borderless);
    }

    #[test]
    fn a_window_with_a_title_bar_is_windowed() {
        // WS_OVERLAPPEDWINDOW and WS_VISIBLE, a window of 1280x800 on the monitor.
        let g = Geometry { window: rect(100, 100, 1380, 900), monitor: MONITOR, style: 0x14cf_0000 };
        assert_eq!(g.mode(), DisplayMode::Windowed);
    }

    #[test]
    fn a_maximized_window_with_a_title_bar_is_still_windowed() {
        let g = Geometry { window: rect(-8, -8, 3448, 1400), monitor: MONITOR, style: 0x15cf_0000 };
        assert_eq!(g.mode(), DisplayMode::Windowed);
    }

    #[test]
    fn a_popup_that_does_not_cover_the_monitor_is_other() {
        let g = Geometry { window: rect(0, 0, 1920, 1080), monitor: MONITOR, style: 0x940a_0000 };
        assert_eq!(g.mode(), DisplayMode::Other);
    }

    #[test]
    fn a_window_with_no_popup_bit_and_no_caption_is_other() {
        let g = Geometry { window: rect(0, 0, 3440, 1440), monitor: MONITOR, style: 0x140a_0000 };
        assert_eq!(g.mode(), DisplayMode::Other);
    }

    #[test]
    fn a_window_on_a_second_monitor_uses_its_own_monitor() {
        let second = rect(3440, 0, 5360, 1080);
        let g = Geometry { window: second, monitor: second, style: 0x940a_0000 };
        assert_eq!(g.mode(), DisplayMode::Borderless);
    }

    #[test]
    fn one_missing_caption_bit_is_not_a_title_bar() {
        // WS_BORDER without WS_DLGFRAME, and a popup that covers the monitor.
        let g = Geometry { window: MONITOR, monitor: MONITOR, style: WS_POPUP.0 | WS_BORDER.0 };
        assert_eq!(g.mode(), DisplayMode::Borderless);
    }
}
