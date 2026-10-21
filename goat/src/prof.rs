//! A tiny scope profiler for the main systems (`let _p = prof::scope("name");`): total time per
//! scope, printed as milliseconds per frame at the end of a test run (performance checks).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

static TOTALS: Mutex<Option<HashMap<&'static str, f64>>> = Mutex::new(None);

pub struct Scope(&'static str, Instant);

pub fn scope(name: &'static str) -> Scope {
    Scope(name, Instant::now())
}

impl Drop for Scope {
    fn drop(&mut self) {
        let ms = self.1.elapsed().as_secs_f64() * 1000.0;
        if let Ok(mut t) = TOTALS.lock() {
            *t.get_or_insert_with(HashMap::new).entry(self.0).or_default() += ms;
        }
    }
}

/// Clear the totals (after loading).
pub fn reset() {
    if let Ok(mut t) = TOTALS.lock() {
        *t = None;
    }
}

/// Scopes by total time, as ms per frame over `frames`.
pub fn report(frames: u32) -> Vec<(&'static str, f64)> {
    let mut v: Vec<(&'static str, f64)> = TOTALS.lock().ok().and_then(|t| t.clone()).unwrap_or_default().into_iter().map(|(k, ms)| (k, ms / frames.max(1) as f64)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}
