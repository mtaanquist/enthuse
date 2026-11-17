//! Settings screen (#69). Fills `SettingsModel` in both windows and saves
//! each change as soon as it is made.

use std::cell::RefCell;
use std::rc::Rc;

use enthuse_core::settings::override_row;
use enthuse_core::source::SourcePath;
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::watched::{
    FRACTION_STEPS, REMAINING_STEPS, WatchedMode, WatchedRule, percent_label, remaining_label, step,
};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::browser::Browser;
use crate::{AppWindow, BrowserWindow, OverrideItem, SettingsModel};

pub struct Settings {
    main: Weak<AppWindow>,
    side: RefCell<Option<Weak<BrowserWindow>>>,
    store: Rc<Store>,
    browser: Rc<Browser>,
    /// The folders listed under "Folder settings", in order.
    overrides: RefCell<Vec<(SourceId, SourcePath)>>,
    /// Called after every saved change.
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

/// Connects a window's Settings callbacks to `$settings`.
macro_rules! wire_callbacks {
    ($window:expr, $settings:expr) => {{
        let model = $window.global::<SettingsModel>();
        let s = $settings.clone();
        model.on_set_resume(move |on| s.save(|store| store.set_default_resume(on)));
        let s = $settings.clone();
        model.on_set_auto_next(move |on| s.save(|store| store.set_auto_next(on)));
        let s = $settings.clone();
        model.on_set_network_thumbnails(move |on| s.save(|store| store.set_network_thumbnails(on)));
        let s = $settings.clone();
        model.on_set_watched_mode(move |i| {
            s.change_rule(|r| r.mode = WatchedMode::from_index(i.max(0) as usize))
        });
        let s = $settings.clone();
        model.on_step_percent(move |d| {
            s.change_rule(|r| r.fraction = step(&FRACTION_STEPS, r.fraction, d))
        });
        let s = $settings.clone();
        model.on_step_remaining(move |d| {
            s.change_rule(|r| r.remaining = step(&REMAINING_STEPS, r.remaining, d))
        });
        let s = $settings.clone();
        model.on_clear_override(move |i| s.clear_override(i as usize));
        let s = $settings.clone();
        model.on_refresh(move || s.refresh());
        model.set_version(env!("CARGO_PKG_VERSION").into());
    }};
}

impl Settings {
    pub fn new(window: &AppWindow, store: Rc<Store>, browser: Rc<Browser>) -> Rc<Self> {
        let this = Rc::new(Self {
            main: window.as_weak(),
            side: Default::default(),
            store,
            browser,
            overrides: Default::default(),
            listeners: Default::default(),
        });
        wire_callbacks!(window, this);
        this.refresh();
        this
    }

    /// Lets the browser window show Settings too.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
        *self.side.borrow_mut() = Some(window.as_weak());
        self.refresh();
    }

    /// Calls `f` after each change, e.g. so playback picks it up.
    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Runs `f` with the Settings model of each open window.
    fn each_model(&self, f: impl Fn(SettingsModel<'_>)) {
        if let Some(w) = self.main.upgrade() {
            f(w.global::<SettingsModel>());
        }
        if let Some(w) = self.side.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(w.global::<SettingsModel>());
        }
    }

    /// Saves one change, then shows the stored state again (so a failed
    /// save shows the old value) and lets the browser pick it up.
    fn save(&self, f: impl FnOnce(&Store) -> enthuse_core::storage::Result<()>) {
        if let Err(err) = f(&self.store) {
            self.browser
                .show_error(&format!("Cannot save this setting: {err}"));
        }
        self.refresh();
        self.browser.settings_changed();
        for f in self.listeners.borrow().iter() {
            f();
        }
    }

    fn change_rule(&self, f: impl FnOnce(&mut WatchedRule)) {
        // Never save defaults over a rule that could not be read.
        let mut rule = match self.store.watched_rule() {
            Ok(rule) => rule,
            Err(err) => {
                self.browser
                    .show_error(&format!("Cannot read this setting: {err}"));
                return;
            }
        };
        f(&mut rule);
        self.save(|store| store.set_watched_rule(&rule));
    }

    fn clear_override(&self, index: usize) {
        let Some((id, path)) = self.overrides.borrow().get(index).cloned() else {
            return;
        };
        self.save(|store| store.set_folder_resume(id, &path, None));
    }

    /// Loads every setting from the database into the screen.
    pub fn refresh(&self) {
        let defaults = self.store.default_playback_settings().unwrap_or_default();
        let auto_next = self.store.auto_next().unwrap_or(true);
        let network = self.store.network_thumbnails().unwrap_or(true);
        let rule = self.store.watched_rule().unwrap_or_default();

        let names: Vec<(SourceId, String)> = self
            .store
            .sources()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect();
        let mut listed = Vec::new();
        let mut items = Vec::new();
        for (id, path, o) in self.store.all_folder_overrides().unwrap_or_default() {
            let Some((_, name)) = names.iter().find(|(n, _)| *n == id) else {
                continue;
            };
            let (folder, detail) = override_row(name, &path, &o);
            items.push(OverrideItem {
                folder: folder.into(),
                detail: detail.into(),
            });
            listed.push((id, path));
        }
        *self.overrides.borrow_mut() = listed;

        let first = |steps: &[f64]| steps.first().copied().unwrap_or_default();
        let last = |steps: &[f64]| steps.last().copied().unwrap_or_default();
        self.each_model(|m| {
            m.set_resume(defaults.resume);
            m.set_auto_next(auto_next);
            m.set_network_thumbnails(network);
            m.set_watched_mode(rule.mode.index() as i32);
            m.set_watched_percent(percent_label(rule.fraction).into());
            m.set_watched_remaining(remaining_label(rule.remaining).into());
            m.set_watched_summary(rule.summary().into());
            m.set_percent_down(rule.fraction > first(&FRACTION_STEPS));
            m.set_percent_up(rule.fraction < last(&FRACTION_STEPS));
            m.set_remaining_down(rule.remaining > first(&REMAINING_STEPS));
            m.set_remaining_up(rule.remaining < last(&REMAINING_STEPS));
            m.set_overrides(ModelRc::new(VecModel::from(items.clone())));
        });
    }
}
