//! Output format modules.

pub mod colors;
pub mod gif;
pub mod image;

pub use self::gif::GifRecorder;
pub use self::image::{Screenshot, ScreenshotConfig};
