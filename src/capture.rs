mod session;

#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;
use image::RgbaImage;

use crate::image_rect::ImageRect;
use crate::window::{ClientPoint, WindowTarget};

pub use session::CaptureSessionManager;

#[cfg(target_os = "windows")]
use windows::WindowsCapture as PlatformCapture;

pub struct CaptureSource {
    platform: PlatformCapture,
}

pub struct CaptureRegion<'a> {
    source: &'a mut CaptureSource,
    rect: ImageRect,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayColorInfo {
    pub hdr_active: bool,
    pub sdr_white_level: u32,
}

pub(crate) trait Rgba16fConverter {
    fn convert_row(&self, source: &[u8], destination: &mut [u8]);

    #[cfg(feature = "diagnostics")]
    fn diagnostic_tag(&self) -> &str;
}

impl CaptureSource {
    pub fn new_for_window_target(target: &WindowTarget) -> Result<Self> {
        Ok(Self {
            platform: PlatformCapture::new(target)?,
        })
    }

    pub fn try_reuse_for_window_target(&mut self, target: &WindowTarget) -> bool {
        self.platform.try_reuse(target)
    }

    pub fn output_size(&self) -> (u32, u32) {
        self.platform.output_size()
    }

    pub fn display_color_info(&self) -> DisplayColorInfo {
        self.platform.display_color_info()
    }

    fn capture_region(
        &mut self,
        client_roi: ImageRect,
        converter: &dyn Rgba16fConverter,
    ) -> Result<RgbaImage> {
        self.platform.capture_region(client_roi, converter)
    }

    pub fn region(&mut self, rect: ImageRect) -> CaptureRegion<'_> {
        CaptureRegion { source: self, rect }
    }
}

impl CaptureRegion<'_> {
    pub(crate) fn capture(&mut self, converter: &dyn Rgba16fConverter) -> Result<RgbaImage> {
        self.source.capture_region(self.rect, converter)
    }

    pub fn map_to_client(&self, local: (u32, u32)) -> ClientPoint {
        ClientPoint {
            x: self.rect.x + local.0,
            y: self.rect.y + local.1,
        }
    }
}
