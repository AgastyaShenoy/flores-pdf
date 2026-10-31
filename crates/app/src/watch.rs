//! Notices when an open file changes on disk, so it can be read again: LaTeX and other tools
//! rewrite a PDF while it's open. One thread looks at the files' size and modification time once a
//! second. A change is reported once the file has held still for a check, so a file that's still
//! being written isn't read half-done.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const EVERY: Duration = Duration::from_secs(1);

type Stamp = Option<(SystemTime, u64)>;

pub struct Watcher {
    files: Arc<Mutex<Vec<PathBuf>>>,
}

impl Watcher {
    /// `changed` runs on the watching thread with each file that changed.
    pub fn new(changed: impl Fn(PathBuf) + Send + 'static) -> Self {
        let files = Arc::new(Mutex::new(Vec::new()));
        let watched = files.clone();
        let _ = std::thread::Builder::new().name("watch".into()).stack_size(64 << 10).spawn(move || {
            // per file: what it was when last read, and a change seen once but not yet settled
            let mut known: HashMap<PathBuf, (Stamp, Option<Stamp>)> = HashMap::new();
            loop {
                std::thread::sleep(EVERY);
                let files = watched.lock().unwrap().clone();
                known.retain(|path, _| files.contains(path));
                for path in files {
                    let now = stamp(&path);
                    let (seen, pending) = known.entry(path.clone()).or_insert((now, None));
                    if now == *seen {
                        *pending = None;
                    } else if *pending == Some(now) && now.is_some() {
                        // the same as a second ago: done changing
                        (*seen, *pending) = (now, None);
                        changed(path);
                    } else {
                        *pending = Some(now);
                    }
                }
            }
        });
        Self { files }
    }

    /// Sets the files to watch. A file new to the list starts from how it is now.
    pub fn watch(&self, paths: Vec<PathBuf>) {
        *self.files.lock().unwrap() = paths;
    }
}

fn stamp(path: &PathBuf) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}
