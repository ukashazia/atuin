use super::{ClipboardBackend, ClipboardError};

#[cfg(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
pub struct ArboardBackend(arboard::Clipboard);

#[cfg(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
impl ArboardBackend {
    pub fn new() -> Result<Self, ClipboardError> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|error| ClipboardError(error.to_string()))
    }
}

#[cfg(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
impl ClipboardBackend for ArboardBackend {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        self.0
            .get_text()
            .map_err(|error| ClipboardError(error.to_string()))
    }

    fn write_text(&mut self, content: String) -> Result<(), ClipboardError> {
        self.0
            .set_text(content)
            .map_err(|error| ClipboardError(error.to_string()))
    }
}

#[cfg(not(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
pub struct ArboardBackend;

#[cfg(not(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
impl ArboardBackend {
    pub fn new() -> Result<Self, ClipboardError> {
        Err(ClipboardError(
            "system clipboard is unavailable on this platform/build".to_owned(),
        ))
    }
}

#[cfg(not(all(
    feature = "arboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
impl ClipboardBackend for ArboardBackend {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        Err(ClipboardError("system clipboard is unavailable".to_owned()))
    }

    fn write_text(&mut self, _content: String) -> Result<(), ClipboardError> {
        Err(ClipboardError("system clipboard is unavailable".to_owned()))
    }
}
