//! GIF recording for terminal sessions.

use std::path::Path;
use std::time::Duration;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Frame, RgbaImage};

use crate::error::{Result, TermwrightError};
use crate::screen::Screen;

use super::image::{Screenshot, ScreenshotConfig};

/// A GIF recorder that captures terminal frames.
pub struct GifRecorder {
    frames: Vec<(RgbaImage, Duration)>,
    config: ScreenshotConfig,
}

impl GifRecorder {
    /// Create a new GIF recorder with default screenshot config.
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            config: ScreenshotConfig::default(),
        }
    }

    /// Create a new GIF recorder with custom screenshot config.
    pub fn with_config(config: ScreenshotConfig) -> Self {
        Self {
            frames: Vec::new(),
            config,
        }
    }

    /// Add a frame with the given display duration.
    pub fn add_frame(&mut self, screen: &Screen, duration: Duration) -> Result<()> {
        let screenshot = Screenshot::with_config(screen.clone(), self.config.clone());
        let image = screenshot.render()?;
        self.frames.push((image, duration));
        Ok(())
    }

    /// Get the number of captured frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Encode all frames to GIF bytes.
    pub fn to_gif(&self) -> Result<Vec<u8>> {
        if self.frames.is_empty() {
            return Err(TermwrightError::Image("no frames to encode".to_string()));
        }

        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new_with_speed(&mut bytes, 10);
            encoder
                .set_repeat(Repeat::Infinite)
                .map_err(|e| TermwrightError::Image(e.to_string()))?;

            for (image, duration) in &self.frames {
                let delay_cs = (duration.as_millis() / 10) as u32;
                let delay = image::Delay::from_saturating_duration(
                    std::time::Duration::from_millis(delay_cs as u64 * 10),
                );
                let frame = Frame::from_parts(image.clone(), 0, 0, delay);
                encoder
                    .encode_frame(frame)
                    .map_err(|e| TermwrightError::Image(e.to_string()))?;
            }
        }

        Ok(bytes)
    }

    /// Save to a GIF file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.to_gif()?;
        std::fs::write(path, bytes).map_err(TermwrightError::Pty)?;
        Ok(())
    }
}
