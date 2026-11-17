//! Folder browser: lists sources, navigates folders and hands videos to the
//! player. Listing runs on a worker thread so a slow or offline share does
//! not freeze the UI.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::source::{Entry, EntryKind, LocalFs, Source, SourceError, SourcePath};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::{AppWindow, BrowserItem, bg};

const KIND_SOURCE: i32 = 0;
const KIND_FOLDER: i32 = 1;
const KIND_VIDEO: i32 = 2;

/// Called when the user picks a video.
pub type PlayHandler = Box<dyn Fn(Arc<dyn Source>, SourcePath)>;

struct SourceSlot {
    source: Arc<dyn Source>,
    /// Shown under the name, e.g. the folder path.
    detail: String,
}

/// Where the browser is: the source list, or a folder inside a source.
#[derive(Clone)]
enum Location {
    Sources,
    Folder { source: usize, path: SourcePath },
}

pub struct Browser {
    window: Weak<AppWindow>,
    sources: RefCell<Vec<SourceSlot>>,
    location: RefCell<Location>,
    entries: RefCell<Vec<Entry>>,
    /// Bumped on every navigation so late results from an earlier listing
    /// are ignored.
    generation: RefCell<u64>,
    on_play: RefCell<Option<PlayHandler>>,
}

impl Browser {
    pub fn new(window: &AppWindow) -> Rc<Self> {
        let this = Rc::new(Self {
            window: window.as_weak(),
            sources: Default::default(),
            location: RefCell::new(Location::Sources),
            entries: Default::default(),
            generation: Default::default(),
            on_play: Default::default(),
        });

        let b = this.clone();
        window.on_browser_activate(move |i| b.activate(i as usize));
        let b = this.clone();
        window.on_browser_up(move || b.up());
        let b = this.clone();
        window.on_add_source(move |path| {
            let path = path.trim();
            if !path.is_empty() {
                b.add_local(path);
                let last = b.sources.borrow().len() - 1;
                b.navigate(Location::Folder {
                    source: last,
                    path: SourcePath::root(),
                });
            }
        });

        this.navigate(Location::Sources);
        this
    }

    pub fn set_play_handler(&self, handler: PlayHandler) {
        *self.on_play.borrow_mut() = Some(handler);
    }

    /// Adds a local folder, mount or UNC path as a source. Sources live in
    /// memory for now; storage (#8) persists them.
    pub fn add_local(&self, path: &str) {
        let source = LocalFs::new(path);
        self.sources.borrow_mut().push(SourceSlot {
            detail: source.root().display().to_string(),
            source: Arc::new(source),
        });
        if matches!(*self.location.borrow(), Location::Sources) {
            self.show_sources();
        }
    }

    fn activate(self: &Rc<Self>, index: usize) {
        let location = self.location.borrow().clone();
        match location {
            Location::Sources => {
                if index < self.sources.borrow().len() {
                    self.navigate(Location::Folder {
                        source: index,
                        path: SourcePath::root(),
                    });
                }
            }
            Location::Folder { source, .. } => {
                let Some(entry) = self.entries.borrow().get(index).cloned() else {
                    return;
                };
                match entry.kind {
                    EntryKind::Folder => self.navigate(Location::Folder {
                        source,
                        path: entry.path,
                    }),
                    EntryKind::Video => {
                        let src = self.sources.borrow()[source].source.clone();
                        if let Some(play) = self.on_play.borrow().as_ref() {
                            play(src, entry.path);
                        }
                    }
                }
            }
        }
    }

    fn up(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            self.navigate(match path.parent() {
                Some(parent) => Location::Folder {
                    source,
                    path: parent,
                },
                None => Location::Sources,
            });
        }
    }

    fn navigate(self: &Rc<Self>, location: Location) {
        *self.location.borrow_mut() = location.clone();
        self.entries.borrow_mut().clear();
        let generation = {
            let mut g = self.generation.borrow_mut();
            *g += 1;
            *g
        };
        let Some(w) = self.window.upgrade() else {
            return;
        };
        w.set_browser_error(SharedString::new());

        match location {
            Location::Sources => {
                w.set_browser_loading(false);
                self.show_sources();
            }
            Location::Folder { source, path } => {
                let slot = &self.sources.borrow()[source];
                w.set_browser_heading(heading(&slot.source.display_name(), &path).into());
                w.set_browser_can_go_up(true);
                w.set_browser_loading(true);
                w.set_browser_items(ModelRc::default());

                let src = slot.source.clone();
                let this = Rc::downgrade(self);
                bg::spawn(
                    move || src.list(&path),
                    move |result| {
                        // Ignore a listing the user already navigated away from.
                        if let Some(this) = this.upgrade()
                            && *this.generation.borrow() == generation
                        {
                            this.show_listing(result);
                        }
                    },
                );
            }
        }
    }

    fn show_sources(&self) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let items: Vec<_> = self
            .sources
            .borrow()
            .iter()
            .map(|s| BrowserItem {
                label: s.source.display_name().into(),
                detail: s.detail.clone().into(),
                kind: KIND_SOURCE,
            })
            .collect();
        w.set_browser_heading("Sources".into());
        w.set_browser_can_go_up(false);
        w.set_browser_items(ModelRc::new(VecModel::from(items)));
    }

    fn show_listing(&self, result: Result<Vec<Entry>, SourceError>) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        w.set_browser_loading(false);
        match result {
            Ok(entries) => {
                let items: Vec<_> = entries
                    .iter()
                    .map(|e| BrowserItem {
                        label: e.name.as_str().into(),
                        detail: SharedString::new(),
                        kind: match e.kind {
                            EntryKind::Folder => KIND_FOLDER,
                            EntryKind::Video => KIND_VIDEO,
                        },
                    })
                    .collect();
                *self.entries.borrow_mut() = entries;
                w.set_browser_items(ModelRc::new(VecModel::from(items)));
            }
            Err(err) => {
                w.set_browser_error(format!("Cannot open this folder: {err}").into());
            }
        }
    }
}

/// "Movies / Action / 2020" style heading for a folder in a source.
fn heading(source_name: &str, path: &SourcePath) -> String {
    std::iter::once(source_name)
        .chain(path.segments())
        .collect::<Vec<_>>()
        .join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_joins_source_and_segments() {
        assert_eq!(heading("Movies", &SourcePath::root()), "Movies");
        let p = SourcePath::parse("Action/2020").unwrap();
        assert_eq!(heading("Movies", &p), "Movies / Action / 2020");
    }
}
