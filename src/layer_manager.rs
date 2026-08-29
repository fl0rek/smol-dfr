use nix::sys::epoll::{Epoll, EpollEvent, EpollFlags};

use crate::config::{Config, ConfigManager, WidgetEntry};
use crate::widgets::{build_widget_layer, FdRegistry, Widget};

pub struct LayerManager {
    layers: [Vec<Box<dyn Widget>>; 2],
    active_layer: usize,
    cfg_mgr: ConfigManager,
    fd_registry: FdRegistry,
    config: Config,
    width: u16,
    widget_entries: [Vec<WidgetEntry>; 2],
}

impl LayerManager {
    /// Create `LayerManager`: loads config, builds widgets, registers fds.
    pub fn new(width: u16, epoll: &Epoll) -> Self {
        let cfg_mgr = ConfigManager::new();
        let (config, widget_entries) = cfg_mgr.load_config(width).expect("Failed to load config");
        let layers = [
            build_widget_layer(
                &widget_entries[0],
                config.workspaces.as_ref(),
                config.volume.as_ref(),
            ),
            build_widget_layer(
                &widget_entries[1],
                config.workspaces.as_ref(),
                config.volume.as_ref(),
            ),
        ];
        let mut fd_registry = FdRegistry::new(10);
        fd_registry.register_all(epoll, &layers);
        // Register config fd with epoll (data=2 to match existing convention)
        epoll
            .add(cfg_mgr.fd(), EpollEvent::new(EpollFlags::EPOLLIN, 2))
            .unwrap();

        Self {
            layers,
            active_layer: 0,
            cfg_mgr,
            fd_registry,
            config,
            width,
            widget_entries,
        }
    }

    pub const fn config(&self) -> &Config {
        &self.config
    }

    pub const fn active_layer(&self) -> usize {
        self.active_layer
    }

    pub const fn switch_layer(&mut self, layer: usize) {
        self.active_layer = layer;
    }

    pub fn active_widgets(&self) -> &[Box<dyn Widget>] {
        &self.layers[self.active_layer]
    }

    pub fn active_widgets_mut(&mut self) -> &mut [Box<dyn Widget>] {
        &mut self.layers[self.active_layer]
    }

    /// Check for config changes. Returns true if config was reloaded.
    /// On reload: rebuilds widgets, re-registers fds, resets active layer.
    pub fn check_config_reload(&mut self, epoll: &Epoll) -> bool {
        if !self
            .cfg_mgr
            .update_config(&mut self.config, &mut self.widget_entries, self.width)
        {
            return false;
        }
        self.active_layer = 0;
        // Remove old widget fds from epoll while widgets are still alive
        // (fd numbers must be valid for epoll_ctl DEL).
        FdRegistry::unregister_all(epoll, &self.layers);
        self.layers = [
            build_widget_layer(
                &self.widget_entries[0],
                self.config.workspaces.as_ref(),
                self.config.volume.as_ref(),
            ),
            build_widget_layer(
                &self.widget_entries[1],
                self.config.workspaces.as_ref(),
                self.config.volume.as_ref(),
            ),
        ];
        self.fd_registry = FdRegistry::new(10);
        self.fd_registry.register_all(epoll, &self.layers);
        true
    }

    /// Call `update()` on all active widgets. Returns true if any changed.
    /// Uses fold instead of `any()` to avoid short-circuiting — every widget
    /// must get its `update()` called (e.g. memory sampling).
    pub fn update(&mut self) -> bool {
        self.layers[self.active_layer]
            .iter_mut()
            .fold(false, |changed, w| w.update() || changed)
    }

    /// Call `poll()` on ALL layers' widgets to drain their eventfds, but only
    /// report changes from the active layer. Both layers must be drained so
    /// that stale signals don't cause unnecessary epoll wakeups.
    pub fn poll(&mut self) -> bool {
        let active = self.active_layer;
        let mut active_changed = false;
        for (li, layer) in self.layers.iter_mut().enumerate() {
            for w in layer.iter_mut() {
                let changed = w.poll();
                if li == active && changed {
                    active_changed = true;
                }
            }
        }
        active_changed
    }

    /// Attempt reconnection on disconnected widgets of BOTH layers, so that a
    /// widget sitting on the inactive layer (e.g. volume with
    /// `MediaLayerDefault = false`) recovers from a service restart without the
    /// user having to switch layers. Only reconnections on the active layer
    /// report a redraw, mirroring `poll()`.
    pub fn reconnect(&mut self) -> bool {
        let active = self.active_layer;
        let mut active_changed = false;
        for (li, layer) in self.layers.iter_mut().enumerate() {
            for w in layer.iter_mut() {
                if !w.is_connected() && w.try_connect() && li == active {
                    active_changed = true;
                }
            }
        }
        active_changed
    }

    /// Whether any widget on either layer is disconnected.
    pub fn any_disconnected(&self) -> bool {
        self.layers
            .iter()
            .flat_map(|layer| layer.iter())
            .any(|w| !w.is_connected())
    }

    /// Whether any active widget needs blink redraws.
    pub fn needs_blink(&self) -> bool {
        self.layers[self.active_layer]
            .iter()
            .any(|w| w.needs_blink())
    }

    /// Whether any active widget needs faster refresh (seconds-level).
    pub fn min_refresh_interval_ms(&self) -> Option<u32> {
        self.layers[self.active_layer]
            .iter()
            .filter_map(|w| w.refresh_interval_ms())
            .min()
    }

    /// Get window title from workspace widget if any.
    pub fn window_title(&self) -> String {
        for widget in &self.layers[self.active_layer] {
            if let Some(title) = widget.window_title() {
                return title;
            }
        }
        String::new()
    }

    /// Focus a workspace by id (delegates to workspace widget).
    pub fn focus_workspace(&self, id: u64) {
        for widget in &self.layers[self.active_layer] {
            widget.focus_workspace_if_applicable(id);
        }
    }

    /// Collect all key actions from all widget entries (for uinput registration).
    pub fn all_key_actions(&self) -> Vec<input_linux::Key> {
        self.widget_entries
            .iter()
            .flat_map(|layer| layer.iter())
            .flat_map(|entry| entry.action.iter().copied())
            .collect()
    }
}
