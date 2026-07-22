use std::fmt;

#[derive(Debug)]
pub struct ClipboardError(pub String);

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClipboardError {}

pub trait ClipboardBackend: Send + 'static {
    fn read_text(&mut self) -> Result<String, ClipboardError>;
    fn write_text(&mut self, content: String) -> Result<(), ClipboardError>;
}

mod arboard;
pub use arboard::ArboardBackend;
