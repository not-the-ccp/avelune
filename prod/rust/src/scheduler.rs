//! Small bounded scheduling helpers; no unsafe shared-memory concurrency is used.
use crate::config::ThreadPolicy;

/// Resolves a thread policy to a bounded worker count.
pub fn worker_count(policy: ThreadPolicy, useful_tasks: usize) -> usize {
    if useful_tasks <= 1 {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    match policy {
        ThreadPolicy::Single => 1,
        ThreadPolicy::Auto => available.min(useful_tasks).max(1),
        ThreadPolicy::Max(n) => n.max(1).min(available).min(useful_tasks),
    }
}

/// Reusable bounded scheduler owned by a stateful codec instance.
///
/// Native builds use a private Rayon pool rather than the process-global pool. WASM baseline
/// builds intentionally remain single-threaded; browser threads are a separate opt-in artifact.
pub struct Scheduler {
    workers: usize,
    #[cfg(not(target_arch = "wasm32"))]
    pool: Option<rayon::ThreadPool>,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("workers", &self.workers)
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// Builds a scheduler bounded by the number of useful plane tasks.
    pub fn new(policy: ThreadPolicy) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let workers = worker_count(policy, 3);
            let pool = (workers > 1)
                .then(|| {
                    rayon::ThreadPoolBuilder::new()
                        .num_threads(workers)
                        .thread_name(|i| format!("avelune-prod-{i}"))
                        .build()
                        .ok()
                })
                .flatten();
            Self { workers, pool }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = policy;
            Self { workers: 1 }
        }
    }

    /// Actual reusable worker count. A pool construction failure degrades safely to one worker.
    pub fn workers(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.pool.is_some() { self.workers } else { 1 }
        }
        #[cfg(target_arch = "wasm32")]
        {
            1
        }
    }

    /// Executes three independent jobs, using this instance's private pool when available.
    pub fn run_three<A, B, C, FA, FB, FC>(&self, fa: FA, fb: FB, fc: FC) -> (A, B, C)
    where
        A: Send,
        B: Send,
        C: Send,
        FA: FnOnce() -> A + Send,
        FB: FnOnce() -> B + Send,
        FC: FnOnce() -> C + Send,
    {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(pool) = &self.pool {
            return pool.install(|| {
                let (a, (b, c)) = rayon::join(fa, || rayon::join(fb, fc));
                (a, b, c)
            });
        }
        (fa(), fb(), fc())
    }
}
