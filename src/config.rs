use anyhow::Error;
use input_linux::Key;
use nix::{
    errno::Errno,
    sys::inotify::{AddWatchFlags, InitFlags, Inotify, InotifyEvent, WatchDescriptor},
};
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer,
};
use std::{fmt, fs::read_to_string, os::fd::AsFd, path::Path};

fn parse_color_str(s: &str) -> Option<(f64, f64, f64)> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
}

#[derive(Clone)]
pub struct WorkspacesConfig {
    pub provider: Option<String>,
    pub active_color: (f64, f64, f64),
    pub urgent_color: (f64, f64, f64),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WorkspacesConfigProxy {
    provider: Option<String>,
    active_color: Option<String>,
    urgent_color: Option<String>,
}

impl From<WorkspacesConfigProxy> for WorkspacesConfig {
    fn from(p: WorkspacesConfigProxy) -> Self {
        Self {
            provider: p.provider,
            active_color: p.active_color
                .and_then(|c| parse_color_str(&c))
                .unwrap_or((0.149, 0.545, 0.824)), // solarized blue
            urgent_color: p.urgent_color
                .and_then(|c| parse_color_str(&c))
                .unwrap_or((0.863, 0.196, 0.184)), // solarized red
        }
    }
}

#[derive(Clone)]
pub struct VolumeConfig {
    pub pulse_server: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeConfigProxy {
    pulse_server: Option<String>,
}

impl From<VolumeConfigProxy> for VolumeConfig {
    fn from(p: VolumeConfigProxy) -> Self {
        Self {
            pulse_server: p.pulse_server,
        }
    }
}

const LOCAL_CFG_PATH: &str = "config.toml";
const SYSTEM_CFG_PATH: &str = "/etc/smol-dfr/config.toml";

fn user_cfg_path() -> &'static str {
    if Path::new(LOCAL_CFG_PATH).exists() {
        LOCAL_CFG_PATH
    } else {
        SYSTEM_CFG_PATH
    }
}

pub struct Config {
    pub show_button_outlines: bool,
    pub enable_pixel_shift: bool,
    pub adaptive_brightness: bool,
    pub active_brightness: u32,
    pub font_family: String,
    pub font_size: f32,
    pub font_bold: bool,
    pub font_italic: bool,
    pub workspaces: Option<WorkspacesConfig>,
    pub volume: Option<VolumeConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ConfigProxy {
    media_layer_default: Option<bool>,
    show_button_outlines: Option<bool>,
    enable_pixel_shift: Option<bool>,
    font_family: Option<String>,
    font_size: Option<f64>,
    font_style: Option<String>,
    adaptive_brightness: Option<bool>,
    active_brightness: Option<u32>,
    primary_layer_keys: Option<Vec<WidgetEntry>>,
    media_layer_keys: Option<Vec<WidgetEntry>>,
    workspaces: Option<WorkspacesConfigProxy>,
    volume: Option<VolumeConfigProxy>,
}

fn array_or_single<'de, D>(deserializer: D) -> Result<Vec<Key>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ArrayOrSingle;

    impl<'de> Visitor<'de> for ArrayOrSingle {
        type Value = Vec<Key>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("string or array of strings")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<Key>, E> {
            Ok(vec![Deserialize::deserialize(
                de::value::BorrowedStrDeserializer::new(value),
            )?])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Vec<Key>, A::Error> {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(ArrayOrSingle)
}

fn parse_hex_color<'de, D>(deserializer: D) -> Result<Option<(f64, f64, f64)>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            let hex = s.strip_prefix('#').unwrap_or(&s);
            if hex.len() != 6 {
                return Err(de::Error::custom("Color must be 6 hex digits, e.g. \"FF8800\""));
            }
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(de::Error::custom)?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(de::Error::custom)?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(de::Error::custom)?;
            Ok(Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)))
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(tag = "Type", rename_all = "snake_case")]
pub enum WidgetConfig {
    #[serde(rename_all = "PascalCase")]
    Text { text: String },
    #[serde(rename_all = "PascalCase")]
    Icon {
        icon: String,
        theme: Option<String>,
    },
    #[serde(rename_all = "PascalCase")]
    Time {
        format: String,
        locale: Option<String>,
    },
    #[serde(rename_all = "PascalCase")]
    Battery { mode: String },
    Temperature,
    #[serde(rename = "load_avg")]
    LoadAvg,
    #[serde(rename_all = "PascalCase")]
    Memory {
        sample_interval: Option<u32>,
        graph_window: Option<u32>,
    },
    Workspaces,
    #[serde(rename = "window_title")]
    WindowTitle,
    Volume,
    Spacer,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct WidgetEntry {
    #[serde(default, deserialize_with = "array_or_single")]
    pub action: Vec<Key>,
    pub width: Option<f64>,
    #[serde(default, deserialize_with = "parse_hex_color")]
    pub color: Option<(f64, f64, f64)>,
    #[serde(flatten)]
    pub widget: WidgetConfig,
}

/// Subset proxy for parsing only global settings (no layer keys).
/// Used as fallback when system config has old-format layer entries.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct BaseConfigProxy {
    media_layer_default: Option<bool>,
    show_button_outlines: Option<bool>,
    enable_pixel_shift: Option<bool>,
    font_family: Option<String>,
    font_size: Option<f64>,
    font_style: Option<String>,
    adaptive_brightness: Option<bool>,
    active_brightness: Option<u32>,
}

impl BaseConfigProxy {
    fn into_config_proxy(self) -> ConfigProxy {
        ConfigProxy {
            media_layer_default: self.media_layer_default,
            show_button_outlines: self.show_button_outlines,
            enable_pixel_shift: self.enable_pixel_shift,
            font_family: self.font_family,
            font_size: self.font_size,
            font_style: self.font_style,
            adaptive_brightness: self.adaptive_brightness,
            active_brightness: self.active_brightness,
            primary_layer_keys: None,
            media_layer_keys: None,
            workspaces: None,
            volume: None,
        }
    }
}

fn load_config(width: u16) -> Result<(Config, [Vec<WidgetEntry>; 2]), String> {
    // Parse system config -- try full ConfigProxy first, fall back to globals-only
    // if layer keys use old format (pre-WidgetConfig boolean-flag style).
    let sys_str = read_to_string("/usr/share/smol-dfr/config.toml").unwrap();
    let mut base = match toml::from_str::<ConfigProxy>(&sys_str) {
        Ok(cfg) => cfg,
        Err(_) => {
            // Old-format system config: parse only global settings, ignore layer keys
            eprintln!("Note: system config uses legacy format, layer keys from user config required");
            toml::from_str::<BaseConfigProxy>(&sys_str)
                .map(|b| b.into_config_proxy())
                .map_err(|e| format!("Failed to parse system config: {e}"))?
        }
    };
    let user = read_to_string(user_cfg_path())
        .map_err::<Error, _>(|e| e.into())
        .and_then(|r| Ok(toml::from_str::<ConfigProxy>(&r)?));
    match &user {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Warning: failed to parse user config: {e}, using base config only");
        }
    }
    if let Ok(user) = user {
        base.media_layer_default = user.media_layer_default.or(base.media_layer_default);
        base.show_button_outlines = user.show_button_outlines.or(base.show_button_outlines);
        base.enable_pixel_shift = user.enable_pixel_shift.or(base.enable_pixel_shift);
        base.font_family = user.font_family.or(base.font_family);
        base.font_size = user.font_size.or(base.font_size);
        base.font_style = user.font_style.or(base.font_style);
        base.adaptive_brightness = user.adaptive_brightness.or(base.adaptive_brightness);
        base.media_layer_keys = user.media_layer_keys.or(base.media_layer_keys);
        base.primary_layer_keys = user.primary_layer_keys.or(base.primary_layer_keys);
        base.active_brightness = user.active_brightness.or(base.active_brightness);
        base.workspaces = user.workspaces.or(base.workspaces);
        base.volume = user.volume.or(base.volume);
    };
    let mut media_layer_keys = base.media_layer_keys
        .ok_or("missing MediaLayerKeys in config")?;
    let mut primary_layer_keys = base.primary_layer_keys
        .ok_or("missing PrimaryLayerKeys in config")?;
    if width >= 2170 {
        for layer in [&mut media_layer_keys, &mut primary_layer_keys] {
            layer.insert(
                0,
                WidgetEntry {
                    action: vec![Key::Esc],
                    width: None,
                    color: None,
                    widget: WidgetConfig::Text { text: "esc".into() },
                },
            );
        }
    }
    let media_layer_default = base.media_layer_default
        .ok_or("missing MediaLayerDefault in config")?;
    let button_layers = if media_layer_default {
        [media_layer_keys, primary_layer_keys]
    } else {
        [primary_layer_keys, media_layer_keys]
    };
    let font_style = base.font_style.as_deref().unwrap_or("");
    let font_bold = font_style.split_whitespace().any(|w| w.eq_ignore_ascii_case("bold"));
    let font_italic = font_style.split_whitespace().any(|w| w.eq_ignore_ascii_case("italic"));
    // Default bold to true if no FontStyle was specified (matches the ":bold" default FontTemplate)
    let font_bold = if base.font_style.is_none() { true } else { font_bold };

    let cfg = Config {
        show_button_outlines: base.show_button_outlines
            .ok_or("missing ShowButtonOutlines in config")?,
        enable_pixel_shift: base.enable_pixel_shift
            .ok_or("missing EnablePixelShift in config")?,
        adaptive_brightness: base.adaptive_brightness
            .ok_or("missing AdaptiveBrightness in config")?,
        active_brightness: base.active_brightness
            .ok_or("missing ActiveBrightness in config")?,
        font_family: base.font_family.unwrap_or_default(),
        font_size: base.font_size.unwrap_or(20.0) as f32,
        font_bold,
        font_italic,
        workspaces: base.workspaces.map(Into::into),
        volume: base.volume.map(Into::into),
    };
    Ok((cfg, button_layers))
}

pub struct ConfigManager {
    inotify_fd: Inotify,
    watch_desc: Option<WatchDescriptor>,
    had_error: bool,
}

fn arm_inotify(inotify_fd: &Inotify) -> Option<WatchDescriptor> {
    let flags = AddWatchFlags::IN_MOVED_TO | AddWatchFlags::IN_CLOSE | AddWatchFlags::IN_ONESHOT;
    match inotify_fd.add_watch(user_cfg_path(), flags) {
        Ok(wd) => Some(wd),
        Err(Errno::ENOENT) => None,
        Err(e) => {
            eprintln!("Warning: inotify add_watch failed: {e}");
            None
        }
    }
}

impl ConfigManager {
    pub fn new() -> ConfigManager {
        let inotify_fd = Inotify::init(InitFlags::IN_NONBLOCK).unwrap();
        let watch_desc = arm_inotify(&inotify_fd);
        ConfigManager {
            inotify_fd,
            watch_desc,
            had_error: false,
        }
    }
    pub fn load_config(&self, width: u16) -> Result<(Config, [Vec<WidgetEntry>; 2]), String> {
        load_config(width)
    }
    pub fn update_config(
        &mut self,
        cfg: &mut Config,
        layers: &mut [Vec<WidgetEntry>; 2],
        width: u16,
    ) -> bool {
        if self.watch_desc.is_none() {
            self.watch_desc = arm_inotify(&self.inotify_fd);
            return false;
        }
        match self.inotify_fd.read_events() {
            Err(Errno::EAGAIN) => false,
            r => self.handle_events(cfg, layers, width, r),
        }
    }
    #[cold]
    fn handle_events(&mut self, cfg: &mut Config, layers: &mut [Vec<WidgetEntry>; 2], width: u16, evts: Result<Vec<InotifyEvent>, Errno>) -> bool {
        let mut ret = false;
        for evt in evts.unwrap_or_default() {
            if Some(evt.wd) != self.watch_desc {
                continue;
            }
            match load_config(width) {
                Ok(parts) => {
                    *cfg = parts.0;
                    *layers = parts.1;
                    ret = true;
                    if self.had_error {
                        eprintln!("Config reloaded successfully");
                        self.had_error = false;
                    }
                }
                Err(e) => {
                    eprintln!("Warning: config reload failed: {e}, keeping previous config");
                    self.had_error = true;
                }
            }
            self.watch_desc = arm_inotify(&self.inotify_fd);
        }
        ret
    }
    pub fn fd(&self) -> &impl AsFd {
        &self.inotify_fd
    }
}
