//! The calls to Windows for the routing overlay. They read the window that has the focus: its
//! title and the file name of its program. They send no input, and they read no memory of another
//! process. The router never lists the windows of the desktop.

use super::mode::Geometry;
use std::path::Path;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::{Owned, PWSTR};

/// The file name of the program of a game client. The folder differs between installs.
const CLIENT_EXE: &str = "exefile.exe";

/// The longest path that Windows allows, in UTF-16 units, with the terminating null.
const MAX_IMAGE_PATH: usize = 32_768;

/// The window that has the focus.
#[derive(Clone, Copy, Debug)]
pub struct Foreground(HWND);

/// The window that has the focus now. No window, or a window that Windows does not name, gives `None`.
pub fn foreground() -> Option<Foreground> {
    // SAFETY: The call has no argument. It gives a null handle when no window has the focus.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(Foreground(hwnd))
}

impl Foreground {
    /// The title of the window. A window with no title gives `None`.
    pub fn title(self) -> Option<String> {
        // SAFETY: A window that is gone gives 0, and the call changes nothing.
        let length = unsafe { GetWindowTextLengthW(self.0) };
        let length = usize::try_from(length).ok().filter(|&n| n > 0)?;
        // One more unit holds the terminating null that the call writes.
        let mut buffer = vec![0u16; length + 1];
        // SAFETY: The call writes at most `buffer.len()` units into the slice. The title can change
        // between the two calls, and then the call cuts the text to fit.
        let copied = unsafe { GetWindowTextW(self.0, &mut buffer) };
        let copied = usize::try_from(copied).ok().filter(|&n| n > 0)?;
        Some(String::from_utf16_lossy(&buffer[..copied]))
    }

    /// True when the program of the window is a game client (`exefile.exe`), in any folder. A
    /// failed call gives `false`. The user may not query the process of a program that runs with
    /// more rights, and the overlay does not show for that client.
    pub fn is_eve_client(self) -> bool {
        self.process_id().and_then(image_file_name).is_some_and(|name| name.eq_ignore_ascii_case(CLIENT_EXE))
    }

    /// The window rectangle, the monitor rectangle and the style bits of the window. A failed
    /// call gives `None`.
    pub fn geometry(self) -> Option<Geometry> {
        let mut window = RECT::default();
        // SAFETY: `window` lives longer than the call, and the call writes one `RECT` through the reference.
        unsafe { GetWindowRect(self.0, &mut window) }.ok()?;
        // SAFETY: The call has no pointer argument. With the flag `MONITOR_DEFAULTTONEAREST` it
        // always gives a monitor, also for a window that is on no monitor.
        let monitor = unsafe { MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>().try_into().ok()?, ..Default::default() };
        // SAFETY: `info` lives longer than the call. `cbSize` holds its size, as the call requires.
        unsafe { GetMonitorInfoW(monitor, &mut info) }.ok().ok()?;
        // SAFETY: The call changes nothing. It gives 0 when it fails, and 0 is not a style that a
        // visible window has.
        let style = unsafe { GetWindowLongPtrW(self.0, GWL_STYLE) };
        // The style is a 32-bit value in the low half of the result.
        (style != 0).then_some(Geometry { window, monitor: info.rcMonitor, style: style as u32 })
    }

    fn process_id(self) -> Option<u32> {
        let mut id = 0u32;
        // SAFETY: `id` lives longer than the call, and the call writes one `u32` through the pointer.
        unsafe { GetWindowThreadProcessId(self.0, Some(&raw mut id)) };
        (id != 0).then_some(id)
    }
}

/// The file name of the program of a process, for example `exefile.exe`.
fn image_file_name(process_id: u32) -> Option<String> {
    // SAFETY: The call has no pointer argument. It gives an error for a process that is gone, or
    // that the user has no right to query.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    // SAFETY: `OpenProcess` gave the handle to this code, and nothing else owns it. `Owned` closes it one time.
    let process = unsafe { Owned::new(process) };
    let mut buffer = vec![0u16; MAX_IMAGE_PATH];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: The handle is open with the right to query. `buffer` has room for `length` units. The
    // call writes at most that many, and sets `length` to the count that it wrote, without the null.
    unsafe { QueryFullProcessImageNameW(*process, PROCESS_NAME_WIN32, PWSTR::from_raw(buffer.as_mut_ptr()), &raw mut length) }.ok()?;
    let path = String::from_utf16_lossy(buffer.get(..usize::try_from(length).ok()?)?);
    Path::new(&path).file_name()?.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path query reads the file name of a real process: the test program.
    #[test]
    fn the_file_name_of_this_process_is_the_name_of_its_program() {
        let expected = std::env::current_exe().unwrap().file_name().unwrap().to_str().unwrap().to_owned();
        let name = image_file_name(std::process::id()).expect("the process can query itself");
        assert!(name.eq_ignore_ascii_case(&expected), "{name} and {expected}");
    }

    #[test]
    fn a_process_that_does_not_exist_gives_no_name() {
        assert_eq!(image_file_name(u32::MAX), None);
    }

    /// The test program is not a game client.
    #[test]
    fn the_test_program_is_not_a_client() {
        let name = image_file_name(std::process::id()).unwrap();
        assert!(!name.eq_ignore_ascii_case(CLIENT_EXE));
    }
}
