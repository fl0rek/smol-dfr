use crate::config::Config;
use crate::TIMEOUT_MS;
use anyhow::{anyhow, Result};
use input::event::{
    switch::{Switch, SwitchEvent, SwitchState},
    Event,
};
use std::{
    cmp::min,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

const MAX_DISPLAY_BRIGHTNESS: u32 = 509;
const MAX_TOUCH_BAR_BRIGHTNESS: u32 = 255;
const BRIGHTNESS_DIM_TIMEOUT: i32 = TIMEOUT_MS * 3; // should be a multiple of TIMEOUT_MS
const BRIGHTNESS_OFF_TIMEOUT: i32 = TIMEOUT_MS * 6; // should be a multiple of TIMEOUT_MS
const DIMMED_BRIGHTNESS: u32 = 1;

fn read_attr(path: &Path, attr: &str) -> Option<u32> {
    fs::read_to_string(path.join(attr))
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

fn find_backlight() -> Result<PathBuf> {
    for entry in fs::read_dir("/sys/class/backlight/")? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if ["display-pipe", "228600000.dsi.0", "appletb_backlight"]
            .iter()
            .any(|s| name.contains(s))
        {
            return Ok(entry.path());
        }
    }
    Err(anyhow!("No Touch Bar backlight device found"))
}

fn find_display_backlight() -> Result<PathBuf> {
    for entry in fs::read_dir("/sys/class/backlight/")? {
        let entry = entry?;
        if [
            "apple-panel-bl",
            "gmux_backlight",
            "intel_backlight",
            "acpi_video0",
        ]
        .iter()
        .any(|s| entry.file_name().to_string_lossy().contains(s))
        {
            return Ok(entry.path());
        }
    }
    Err(anyhow!("No Built-in Retina Display backlight device found"))
}

pub struct BacklightManager {
    last_active: Instant,
    max_bl: u32,
    current_bl: u32,
    lid_state: SwitchState,
    bl_file: Option<File>,
    bl_path: Option<PathBuf>,
    display_bl_path: Option<PathBuf>,
    lid_just_opened: bool,
}

impl BacklightManager {
    pub fn new() -> Self {
        let (bl_path, max_bl, current_bl, bl_file) = match find_backlight() {
            Ok(path) => {
                let max_bl = read_attr(&path, "max_brightness").unwrap_or(0);
                let current_bl = read_attr(&path, "brightness").unwrap_or(0);
                let bl_file = OpenOptions::new()
                    .write(true)
                    .open(path.join("brightness"))
                    .map_err(|e| {
                        eprintln!("Warning: failed to open brightness file: {e}");
                    })
                    .ok();
                (Some(path), max_bl, current_bl, bl_file)
            }
            Err(e) => {
                eprintln!("Warning: {e}, brightness control disabled");
                (None, 0, 0, None)
            }
        };

        let display_bl_path = match find_display_backlight() {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("Warning: {e}, adaptive brightness disabled");
                None
            }
        };

        Self {
            bl_file,
            bl_path,
            lid_state: SwitchState::Off,
            max_bl,
            current_bl,
            last_active: Instant::now(),
            display_bl_path,
            lid_just_opened: false,
        }
    }
    fn display_to_touchbar(display: u32, active_brightness: u32) -> u32 {
        let normalized = f64::from(display) / f64::from(MAX_DISPLAY_BRIGHTNESS);
        // Add one so that the touch bar does not turn off
        let adjusted = (normalized.powf(0.5) * f64::from(active_brightness)) as u32 + 1;
        adjusted.min(MAX_TOUCH_BAR_BRIGHTNESS) // Clamp the value to the maximum allowed brightness
    }
    fn set_backlight(&mut self, value: u32) {
        let Some(ref mut file) = self.bl_file else {
            return;
        };
        if let Err(e) = file.write_all(format!("{value}\n").as_bytes()) {
            eprintln!("Warning: backlight write failed: {e}, attempting fd re-open");
            // Try to re-open the brightness file
            if let Some(ref bl_path) = self.bl_path {
                match OpenOptions::new()
                    .write(true)
                    .open(bl_path.join("brightness"))
                {
                    Ok(mut new_file) => {
                        // Retry write once with new fd
                        if let Err(e2) = new_file.write_all(format!("{value}\n").as_bytes()) {
                            eprintln!("Warning: backlight retry write failed: {e2}, disabling brightness control");
                            self.bl_file = None;
                        } else {
                            self.bl_file = Some(new_file);
                        }
                    }
                    Err(e2) => {
                        eprintln!("Warning: backlight fd re-open failed: {e2}, disabling brightness control");
                        self.bl_file = None;
                    }
                }
            } else {
                self.bl_file = None;
            }
        }
    }
    pub fn process_event(&mut self, event: &Event) {
        match event {
            Event::Keyboard(_) | Event::Pointer(_) | Event::Gesture(_) | Event::Touch(_) => {
                self.last_active = Instant::now();
            }
            Event::Switch(SwitchEvent::Toggle(toggle)) => {
                if toggle.switch() == Some(Switch::Lid) {
                    self.lid_state = toggle.switch_state();
                    eprintln!("Lid Switch event: {:?}", self.lid_state);
                    if toggle.switch_state() == SwitchState::Off {
                        self.last_active = Instant::now();
                        self.lid_just_opened = true;
                    }
                }
            }
            _ => {}
        }
    }
    /// Returns true if lid was opened since last call, clears the flag.
    pub const fn take_lid_opened(&mut self) -> bool {
        let val = self.lid_just_opened;
        self.lid_just_opened = false;
        val
    }
    pub fn update_backlight(&mut self, cfg: &Config) {
        if self.bl_file.is_none() {
            return;
        }
        let since_last_active = self.last_active.elapsed().as_millis() as u64;
        let new_bl = min(
            self.max_bl,
            if self.lid_state == SwitchState::On {
                0
            } else if since_last_active < BRIGHTNESS_DIM_TIMEOUT as u64 {
                if cfg.adaptive_brightness {
                    // Read display brightness; fall back to fixed brightness if unavailable
                    let display_bl = self
                        .display_bl_path
                        .as_ref()
                        .and_then(|p| read_attr(p, "brightness"));
                    match display_bl {
                        Some(bl) => Self::display_to_touchbar(bl, cfg.active_brightness),
                        None => cfg.active_brightness,
                    }
                } else {
                    cfg.active_brightness
                }
            } else if since_last_active < BRIGHTNESS_OFF_TIMEOUT as u64 {
                DIMMED_BRIGHTNESS
            } else {
                0
            },
        );
        if self.current_bl != new_bl {
            self.current_bl = new_bl;
            self.set_backlight(self.current_bl);
        }
    }
    pub const fn current_bl(&self) -> u32 {
        self.current_bl
    }
}
