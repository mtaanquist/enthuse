//! Video thumbnails for rows and cards (#68). One worker thread takes files
//! from a [`ThumbQueue`] one at a time and grabs a frame with a headless
//! libmpv, so a slow share never blocks browsing or starves playback.
//! Results come back on the UI thread to whoever listens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use enthuse_core::source::{Source, SourcePath, is_network_location};
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::thumbs::{self, Lookup, ThumbCache, ThumbQueue, cache_key};
use enthuse_player::thumbnail::{GrabError, grab_frame};

/// Longest a single grab may take, e.g. on a slow share.
const GRAB_TIMEOUT: Duration = Duration::from_secs(20);

/// A file to make a thumbnail for.
pub struct Job {
    source: Arc<dyn Source>,
    path: SourcePath,
}

/// Names a file across sources, to match results to rows and cards.
pub fn thumb_id(source: SourceId, path: &SourcePath) -> String {
    format!("{}:{path}", source.0)
}

type Listener = Box<dyn Fn(&str, &slint::Image)>;

struct Shared {
    queue: Mutex<ThumbQueue<Job>>,
    wake: Condvar,
}

pub struct Thumbs {
    shared: Option<Arc<Shared>>,
    store: Rc<Store>,
    /// Results this session: the image file, or `None` when there is none.
    known: RefCell<HashMap<String, Option<PathBuf>>>,
    listeners: RefCell<Vec<Listener>>,
}

thread_local! {
    /// Where the worker's results go; only the UI thread sets or reads it.
    static ACTIVE: RefCell<Weak<Thumbs>> = const { RefCell::new(Weak::new()) };
}

impl Thumbs {
    /// Starts the worker, keeping thumbnails in `cache_dir`. Without one
    /// (no home folder, the self-test) there are no thumbnails.
    pub fn new(store: Rc<Store>, cache_dir: Option<PathBuf>) -> Rc<Self> {
        let shared = cache_dir.map(|dir| {
            let shared = Arc::new(Shared {
                queue: Mutex::new(ThumbQueue::new()),
                wake: Condvar::new(),
            });
            let worker = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("thumbnails".into())
                .spawn(move || run(worker, ThumbCache::new(dir)));
            if let Err(err) = spawned {
                eprintln!("enthuse: cannot start the thumbnail worker: {err}");
            }
            shared
        });
        let this = Rc::new(Self {
            shared,
            store,
            known: Default::default(),
            listeners: Default::default(),
        });
        ACTIVE.with(|a| *a.borrow_mut() = Rc::downgrade(&this));
        this
    }

    /// Calls `f` with the id and image of each thumbnail as it arrives.
    pub fn on_ready(&self, f: impl Fn(&str, &slint::Image) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// The thumbnail for `id`, if one has arrived this session.
    pub fn image(&self, id: &str) -> slint::Image {
        self.known
            .borrow()
            .get(id)
            .and_then(|p| p.as_deref())
            .and_then(|p| slint::Image::load_from_path(p).ok())
            .unwrap_or_default()
    }

    /// Asks for a thumbnail ahead of the folder being browsed, for a card.
    pub fn want(&self, source: SourceId, src: &Arc<dyn Source>, location: &str, path: &SourcePath) {
        let Some(shared) = &self.shared else {
            return;
        };
        let id = thumb_id(source, path);
        if self.known.borrow().contains_key(&id) || !self.allowed(location) {
            return;
        }
        if let Ok(mut q) = shared.queue.lock() {
            q.push_urgent(
                id,
                Job {
                    source: src.clone(),
                    path: path.clone(),
                },
            );
        }
        shared.wake.notify_one();
    }

    /// Asks for thumbnails of `files` in the folder being browsed, in
    /// order, dropping what is left of the previous folder's.
    pub fn want_folder(
        &self,
        source: SourceId,
        src: &Arc<dyn Source>,
        location: &str,
        files: &[SourcePath],
    ) {
        let Some(shared) = &self.shared else {
            return;
        };
        let jobs: Vec<_> = if self.allowed(location) {
            let known = self.known.borrow();
            files
                .iter()
                .map(|path| (thumb_id(source, path), path))
                .filter(|(id, _)| !known.contains_key(id))
                .map(|(id, path)| {
                    let job = Job {
                        source: src.clone(),
                        path: path.clone(),
                    };
                    (id, job)
                })
                .collect()
        } else {
            Vec::new()
        };
        if let Ok(mut q) = shared.queue.lock() {
            q.set_folder(jobs);
        }
        shared.wake.notify_one();
    }

    /// Stops the folder's pending thumbnails, e.g. when leaving it.
    pub fn clear_folder(&self) {
        if let Some(shared) = &self.shared
            && let Ok(mut q) = shared.queue.lock()
        {
            q.set_folder(Vec::new());
        }
    }

    /// Files on network shares get thumbnails only if the setting allows.
    fn allowed(&self, location: &str) -> bool {
        !is_network_location(location) || self.store.network_thumbnails().unwrap_or(true)
    }

    /// A result from the worker.
    fn done(&self, id: String, image: Option<PathBuf>) {
        let loaded = image
            .as_deref()
            .and_then(|p| slint::Image::load_from_path(p).ok());
        self.known.borrow_mut().insert(id.clone(), image);
        if let Some(image) = loaded {
            for f in self.listeners.borrow().iter() {
                f(&id, &image);
            }
        }
    }
}

/// The worker: makes thumbnails until the app quits.
fn run(shared: Arc<Shared>, cache: ThumbCache) {
    if let Err(err) = std::fs::create_dir_all(cache.dir()) {
        eprintln!(
            "enthuse: no thumbnails, cannot create {}: {err}",
            cache.dir().display()
        );
        return;
    }
    if let Err(err) = cache.prune(thumbs::CACHE_LIMIT) {
        eprintln!("enthuse: cannot clean up the thumbnail cache: {err}");
    }
    loop {
        let (id, job) = {
            let Ok(mut q) = shared.queue.lock() else {
                return;
            };
            loop {
                if let Some(next) = q.pop() {
                    break next;
                }
                q = match shared.wake.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        let image = make(&cache, &job);
        let sent = slint::invoke_from_event_loop(move || {
            ACTIVE.with(|a| {
                if let Some(thumbs) = a.borrow().upgrade() {
                    thumbs.done(id, image);
                }
            });
        });
        if sent.is_err() {
            return; // The event loop is gone: the app is quitting.
        }
    }
}

/// The thumbnail for `job`, from the cache or a new frame grab.
fn make(cache: &ThumbCache, job: &Job) -> Option<PathBuf> {
    let url = job.source.playable(&job.path).ok()?;
    let key = cache_key(&url, job.source.stamp(&job.path));
    match cache.lookup(&key) {
        Lookup::Ready(path) => Some(path),
        Lookup::Failed => None,
        Lookup::Missing => {
            let out = cache.image_path(&key);
            match grab_frame(&url, &out, thumbs::WIDTH, GRAB_TIMEOUT) {
                Ok(()) => Some(out),
                // A slow share: try again another time.
                Err(GrabError::TimedOut) => None,
                Err(GrabError::Failed(_)) => {
                    let _ = cache.mark_failed(&key);
                    None
                }
            }
        }
    }
}
