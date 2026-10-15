//! Work that happens later, off the tick, or on another thread, and the rules for each.
//!
//! # The threading rules, because getting these wrong crashes the client
//!
//! - **The main thread is where a mod runs.** `load`, every lifecycle phase, `tick` and the
//!   render phases all arrive on the thread that owns the client's frame. Host calls —
//!   settings, the overlay, `mods::`, the event bus — are only safe there.
//! - **Render work belongs to the render phase.** [`schedule_render_thread`] and
//!   [`RenderThreadExecutor`] queue a task that runs inside the frame's render phase. There
//!   is no second render thread to hand work to in this client, and a task queued this way
//!   is not running while the world is being mutated.
//! - **Pure work belongs off the thread.** [`schedule_async`], [`schedule_background`] and
//!   [`WorkerPool`] run on real worker threads. They must not call host APIs, read the
//!   client's state, or touch anything a mod registered; they compute and hand a result
//!   back through [`TaskResult`].
//! - **The network thread is out of reach.** The client owns it and exposes no way to post
//!   to it, so [`schedule_network_thread`] refuses rather than pretending. That is the one
//!   honest `None` here.
//! - **Never block the main thread.** [`wait`] and [`join`] are bounded on purpose. A mod
//!   that needs an answer from a background task asks for it at the start of the next tick
//!   and moves on.
//!
//! ```
//! use cinnaloader::prelude::*;
//! use std::time::Duration;
//!
//! let beat = tasks::schedule_repeating(Duration::from_millis(1500), || info!("still here"));
//! let handle = tasks::schedule_after(Duration::from_secs(10), move || {
//!     tasks::cancel(beat);
//! });
//! # let _ = handle;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Which thread a task wants to run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadAffinity {
    /// The client's frame thread, where a mod already runs.
    Main,
    /// The thread that steps the game. The same as [`ThreadAffinity::Main`] in this host.
    Game,
    /// The frame's render phase. The same thread, a different moment.
    Render,
    /// The client's networking thread, which this host cannot reach.
    Network,
    /// A worker thread of its own.
    Background,
    /// Anywhere it can get to; the scheduler decides.
    Any,
}

impl ThreadAffinity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Game => "game",
            Self::Render => "render",
            Self::Network => "network",
            Self::Background => "background",
            Self::Any => "any",
        }
    }

    /// Whether this host can actually deliver to it.
    #[must_use]
    pub fn is_reachable(self) -> bool {
        !matches!(self, Self::Network)
    }
}

/// Where a task sits in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl TaskPriority {
    /// Where it sorts: `Critical` first, `Low` last.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
        }
    }
}

/// How a task is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    /// Waiting for its turn, or for its time.
    Scheduled,
    /// On a thread right now.
    Running,
    /// Finished.
    Done,
    /// Taken off the queue before it ran.
    Cancelled,
    /// Panicked, and was caught.
    Failed,
    /// Waiting for another task to finish first.
    Waiting,
}

/// A handle to a task that was scheduled. Cheap to copy, and enough to cancel it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskHandle {
    id: u64,
    affinity: ThreadAffinity,
    priority: TaskPriority,
}

impl Default for TaskHandle {
    /// The handle that stands for no task at all.
    fn default() -> Self {
        Self {
            id: 0,
            affinity: ThreadAffinity::Main,
            priority: TaskPriority::Normal,
        }
    }
}

impl TaskHandle {
    /// The handle that stands for no task.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether this stands for no task.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.id == 0
    }

    #[must_use]
    pub fn id(self) -> u64 {
        self.id
    }

    #[must_use]
    pub fn affinity(self) -> ThreadAffinity {
        self.affinity
    }

    #[must_use]
    pub fn priority(self) -> TaskPriority {
        self.priority
    }

    #[must_use]
    pub fn status(self) -> TaskStatus {
        status_of(self.id)
    }

    #[must_use]
    pub fn is_done(self) -> bool {
        matches!(
            self.status(),
            TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Failed
        )
    }

    #[must_use]
    pub fn is_cancelled(self) -> bool {
        self.status() == TaskStatus::Cancelled
    }
}

/// The result of a task that produced one. Read it once the task is done; reading it early
/// is not an error, it is just not ready yet.
pub struct TaskResult<T> {
    handle: TaskHandle,
    slot: Arc<Mutex<Option<T>>>,
}

impl<T> TaskResult<T> {
    #[must_use]
    pub fn handle(&self) -> TaskHandle {
        self.handle
    }

    /// Whether the task has put something in the slot.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.slot.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    /// Takes the result, or `None` when the task has not finished or took it already.
    pub fn get(&self) -> Option<T> {
        self.slot.lock().ok().and_then(|mut slot| slot.take())
    }

    /// Looks at the result without taking it.
    #[must_use]
    pub fn peek(&self) -> Option<T>
    where
        T: Clone,
    {
        self.slot.lock().ok().and_then(|slot| slot.clone())
    }
}

/// A flag a task can watch to stop what it is doing.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// The handle, so a token can be made from one that already exists.
    #[must_use]
    pub fn from_handle(handle: TaskHandle) -> Self {
        let token = Self::new();
        TOKENS.with(|tokens| {
            tokens
                .lock()
                .map(|mut tokens| tokens.insert(handle.id, token.clone()))
                .ok();
        });
        token
    }
}

/// A group of tasks that are cancelled together.
#[derive(Debug, Default)]
pub struct JobGroup {
    handles: Mutex<Vec<TaskHandle>>,
}

impl JobGroup {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, handle: TaskHandle) {
        if let Ok(mut handles) = self.handles.lock() {
            handles.push(handle);
        }
    }

    /// Cancels every task in the group that has not run yet or repeats.
    pub fn cancel(&self) -> usize {
        let Ok(handles) = self.handles.lock() else {
            return 0;
        };
        handles.iter().filter(|handle| cancel(**handle)).count()
    }

    #[must_use]
    pub fn handles(&self) -> Vec<TaskHandle> {
        self.handles
            .lock()
            .map(|handles| handles.clone())
            .unwrap_or_default()
    }

    /// Whether every task in the group has finished or been cancelled.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.handles()
            .iter()
            .all(|handle| handle.is_done())
    }
}

/// Where a task can be sent, as a value, for code that picks its executor at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskExecutor {
    MainThread,
    RenderThread,
    Network,
    Background,
    /// A pool of `n` workers.
    Workers(usize),
}

impl TaskExecutor {
    /// Sends `task` there, or refuses when there is nowhere to send it.
    pub fn submit<F: FnOnce() + Send + 'static>(self, task: F) -> Option<TaskHandle> {
        match self {
            Self::MainThread => submit(task),
            Self::RenderThread => schedule_render_thread(task),
            Self::Network => schedule_network_thread(task),
            Self::Background => Some(schedule_background(task)),
            Self::Workers(workers) => WorkerPool::new(workers.max(1)).submit(task),
        }
    }
}

/// The client's main thread, as an executor.
#[derive(Debug, Clone, Copy, Default)]
pub struct MainThreadExecutor;

impl MainThreadExecutor {
    pub fn submit<F: FnOnce() + Send + 'static>(self, task: F) -> Option<TaskHandle> {
        submit(task)
    }
}

/// The render phase, as an executor.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderThreadExecutor;

impl RenderThreadExecutor {
    pub fn submit<F: FnOnce() + Send + 'static>(self, task: F) -> Option<TaskHandle> {
        schedule_render_thread(task)
    }
}

/// The client's network thread, which this host cannot post to.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetworkExecutor;

impl NetworkExecutor {
    /// Always `None`: the client owns its network thread and offers no queue into it.
    pub fn submit<F: FnOnce() + Send + 'static>(self, _task: F) -> Option<TaskHandle> {
        schedule_network_thread(|| {})
    }
}

/// Counters for a mod that wants to see what its scheduling is costing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskMetrics {
    pub scheduled: u64,
    pub ran: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub repeating: u64,
    pub background: u64,
    pub pending: usize,
    pub queue_length: usize,
}

/// A queue of jobs and a few worker threads, for a mod that has real work to spread out.
///
/// The workers are plain threads: nothing in here touches the client, and a job that does
/// would be a bug. Results come back through [`WorkerPool::drain`] on the main thread.
pub struct WorkerPool {
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl WorkerPool {
    /// Starts `workers` threads, each pulling from a shared queue.
    #[must_use]
    pub fn new(workers: usize) -> Self {
        let count = workers.max(1);
        let mut handles = Vec::with_capacity(count);
        for index in 0..count {
            let id = WORKER_IDS.fetch_add(1, Ordering::Relaxed);
            let handle = std::thread::Builder::new()
                .name(format!("cinnaloader-worker-{index}"))
                .spawn(move || {
                    WORKER_THREADS.with(|threads| {
                        if let Ok(mut threads) = threads.lock() {
                            threads.insert(std::thread::current().id(), id);
                        }
                    });
                    worker_loop();
                });
            match handle {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        Self { workers: handles }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.workers.len()
    }

    /// Queues a job. Returns whether there was a worker to take it.
    pub fn submit<F: FnOnce() + Send + 'static>(&self, job: F) -> Option<TaskHandle> {
        if self.workers.is_empty() {
            return None;
        }
        Some(schedule_background(job))
    }

    /// A job whose result is picked up on the main thread.
    #[must_use]
    pub fn submit_with_result<T, F>(&self, job: F) -> TaskResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        submit_with_result(job)
    }
}

/// The queue a mod's tasks wait in.
#[derive(Default)]
pub struct JobQueue {
    tasks: Mutex<Vec<Task>>,
}

impl JobQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.lock().map(|tasks| tasks.len()).unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Runs everything due now, oldest priority first.
    pub fn drain(&self) -> usize {
        let due: Vec<Task> = match self.tasks.lock() {
            Ok(mut tasks) => {
                let now = Instant::now();
                let (due, waiting): (Vec<Task>, Vec<Task>) = tasks
                    .drain(..)
                    .partition(|task| task.due.is_none_or(|due| due <= now));
                *tasks = waiting;
                due
            }
            Err(_) => return 0,
        };
        run_all(due)
    }
}

/// `AsyncResult` is what the wishlist calls a task's result.
pub type AsyncResult<T> = TaskResult<T>;

/// A task's work: something that runs once, or something that runs again each time it is
/// due.
enum Body {
    Once(Box<dyn FnOnce() + Send>),
    Repeat(Box<dyn FnMut() + Send>),
}

struct Task {
    id: u64,
    body: Option<Body>,
    due: Option<Instant>,
    every: Option<Duration>,
    priority: TaskPriority,
    affinity: ThreadAffinity,
    cancelled: Arc<AtomicBool>,
}

static NEXT_TASK: AtomicU64 = AtomicU64::new(1);
static WORKER_IDS: AtomicU64 = AtomicU64::new(1);

fn queue() -> &'static Mutex<Vec<Task>> {
    static QUEUE: OnceLock<Mutex<Vec<Task>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

fn render_queue() -> &'static Mutex<Vec<Task>> {
    static RENDER: OnceLock<Mutex<Vec<Task>>> = OnceLock::new();
    RENDER.get_or_init(|| Mutex::new(Vec::new()))
}

fn statuses() -> &'static Mutex<HashMap<u64, TaskStatus>> {
    static STATUSES: OnceLock<Mutex<HashMap<u64, TaskStatus>>> = OnceLock::new();
    STATUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn metrics() -> &'static Mutex<TaskMetrics> {
    static METRICS: OnceLock<Mutex<TaskMetrics>> = OnceLock::new();
    METRICS.get_or_init(|| Mutex::new(TaskMetrics::default()))
}

fn worker_jobs() -> &'static Mutex<Vec<Task>> {
    static JOBS: OnceLock<Mutex<Vec<Task>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(Vec::new()))
}

fn worker_signal() -> &'static (Mutex<bool>, Condvar) {
    static SIGNAL: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
    SIGNAL.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn threads() -> &'static Mutex<HashMap<String, ThreadId>> {
    static THREADS: OnceLock<Mutex<HashMap<String, ThreadId>>> = OnceLock::new();
    THREADS.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static TOKENS: Mutex<HashMap<u64, CancellationToken>> = Mutex::new(HashMap::new());
    static WORKER_THREADS: Mutex<HashMap<ThreadId, u64>> = Mutex::new(HashMap::new());
}

fn worker_loop() {
    loop {
        let job = next_worker_job();
        let Some(mut task) = job else {
            continue;
        };
        run_body(&mut task);
    }
}

fn next_worker_job() -> Option<Task> {
    let (lock, signal) = worker_signal();
    let mut ready = lock.lock().ok()?;
    while !*ready {
        let waiting = worker_jobs()
            .lock()
            .map(|jobs| !jobs.is_empty())
            .unwrap_or(false);
        if waiting {
            break;
        }
        ready = signal.wait_timeout(ready, Duration::from_millis(50)).ok()?.0;
    }
    *ready = false;
    let mut jobs = worker_jobs().lock().ok()?;
    // Highest priority first, so the reverse of the rank order, popped off the end.
    jobs.sort_by_key(|task| std::cmp::Reverse(task.priority.rank()));
    jobs.pop()
}

/// Runs a task's body once, and records what happened to it. A repeating body stays in the
/// task, so the caller can put it back for its next turn.
fn run_body(task: &mut Task) -> TaskStatus {
    mark(task.id, TaskStatus::Running);
    let caught = match task.body.as_mut() {
        Some(Body::Once(_)) => match task.body.take() {
            Some(Body::Once(run)) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)),
            _ => Ok(()),
        },
        Some(Body::Repeat(run)) => {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        }
        None => Ok(()),
    };
    match caught {
        Ok(()) => {
            mark(task.id, TaskStatus::Done);
            count(|metrics| metrics.ran += 1);
            TaskStatus::Done
        }
        Err(_) => {
            mark(task.id, TaskStatus::Failed);
            count(|metrics| metrics.failed += 1);
            crate::warn!("a scheduled task panicked; it will not run again");
            TaskStatus::Failed
        }
    }
}

fn mark(id: u64, status: TaskStatus) {
    if let Ok(mut statuses) = statuses().lock() {
        statuses.insert(id, status);
    }
}

fn status_of(id: u64) -> TaskStatus {
    if id == 0 {
        return TaskStatus::Cancelled;
    }
    statuses()
        .lock()
        .ok()
        .and_then(|statuses| statuses.get(&id).copied())
        .unwrap_or(TaskStatus::Scheduled)
}

fn count(change: impl FnOnce(&mut TaskMetrics)) {
    if let Ok(mut metrics) = metrics().lock() {
        change(&mut metrics);
    }
}

fn queue_task(task: Task, where_to: Queue) -> TaskHandle {
    let handle = TaskHandle {
        id: task.id,
        affinity: task.affinity,
        priority: task.priority,
    };
    mark(task.id, TaskStatus::Scheduled);
    count(|metrics| {
        metrics.scheduled += 1;
        if task.every.is_some() {
            metrics.repeating += 1;
        }
        if matches!(
            task.affinity,
            ThreadAffinity::Background | ThreadAffinity::Any
        ) {
            metrics.background += 1;
        }
    });
    match where_to {
        Queue::Main => {
            if let Ok(mut queue) = queue().lock() {
                queue.push(task);
            }
        }
        Queue::Render => {
            if let Ok(mut queue) = render_queue().lock() {
                queue.push(task);
            }
        }
        Queue::Worker => {
            if let Ok(mut jobs) = worker_jobs().lock() {
                jobs.push(task);
            }
            let (lock, signal) = worker_signal();
            if let Ok(mut ready) = lock.lock() {
                *ready = true;
                signal.notify_one();
            }
        }
    }
    handle
}

enum Queue {
    Main,
    Render,
    Worker,
}

fn spawn_task(
    affinity: ThreadAffinity,
    priority: TaskPriority,
    due: Option<Instant>,
    every: Option<Duration>,
    run: impl FnOnce() + Send + 'static,
) -> TaskHandle {
    let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = Task {
        id,
        body: Some(Body::Once(Box::new(run))),
        due,
        every,
        priority,
        affinity,
        cancelled,
    };
    let where_to = match affinity {
        ThreadAffinity::Background | ThreadAffinity::Any => Queue::Worker,
        ThreadAffinity::Render => Queue::Render,
        _ => Queue::Main,
    };
    queue_task(task, where_to)
}

/// Runs `task` at the end of this tick, on the thread a mod already runs on.
pub fn schedule(task: impl FnOnce() + Send + 'static) -> Option<TaskHandle> {
    schedule_with(TaskPriority::Normal, ThreadAffinity::Main, task)
}

/// The same, naming the priority.
pub fn schedule_with(
    priority: TaskPriority,
    affinity: ThreadAffinity,
    task: impl FnOnce() + Send + 'static,
) -> Option<TaskHandle> {
    if !affinity.is_reachable() {
        crate::warn!(
            "{} is not reachable from this host, so the task was not scheduled",
            affinity.as_str()
        );
        return None;
    }
    Some(spawn_task(affinity, priority, None, None, task))
}

/// Runs `task` once, after `delay` has passed, checked on every tick.
pub fn schedule_after(delay: Duration, task: impl FnOnce() + Send + 'static) -> TaskHandle {
    spawn_task(
        ThreadAffinity::Main,
        TaskPriority::Normal,
        Some(Instant::now() + delay),
        None,
        task,
    )
}

/// The same as [`schedule_after`], for a duration in whole milliseconds.
pub fn schedule_delayed(millis: u64, task: impl FnOnce() + Send + 'static) -> TaskHandle {
    schedule_after(Duration::from_millis(millis), task)
}

/// Runs `task` on every tick, starting with the next one.
pub fn schedule_every_tick(task: impl FnMut() + Send + 'static) -> TaskHandle {
    let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = Task {
        id,
        body: Some(Body::Repeat(Box::new(task))),
        due: None,
        every: None,
        priority: TaskPriority::Normal,
        affinity: ThreadAffinity::Main,
        cancelled: Arc::clone(&cancelled),
    };
    TOKENS.with(|tokens| {
        if let Ok(mut tokens) = tokens.lock() {
            tokens.insert(
                id,
                CancellationToken {
                    cancelled: Arc::clone(&cancelled),
                },
            );
        }
    });
    queue_task(task, Queue::Main)
}

/// Runs `task` once, `ticks` ticks from now.
pub fn schedule_after_ticks(ticks: u32, task: impl FnOnce() + Send + 'static) -> TaskHandle {
    schedule_after(
        Duration::from_millis(u64::from(ticks) * 50),
        task,
    )
}

/// Runs `task` now and then every `interval`, which the tick checks.
pub fn schedule_repeating(
    interval: Duration,
    task: impl FnMut() + Send + 'static,
) -> TaskHandle {
    let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    let token = CancellationToken {
        cancelled: Arc::clone(&cancelled),
    };
    let task = Task {
        id,
        body: Some(Body::Repeat(Box::new(task))),
        due: Some(Instant::now() + interval),
        every: Some(interval),
        priority: TaskPriority::Normal,
        affinity: ThreadAffinity::Main,
        cancelled,
    };
    TOKENS.with(|tokens| {
        if let Ok(mut tokens) = tokens.lock() {
            tokens.insert(id, token);
        }
    });
    queue_task(task, Queue::Main)
}

/// Runs `task` on a worker thread, now.
pub fn schedule_async(task: impl FnOnce() + Send + 'static) -> TaskHandle {
    spawn_task(
        ThreadAffinity::Background,
        TaskPriority::Normal,
        None,
        None,
        task,
    )
}

/// The same as [`schedule_async`]: a worker thread that is not the client's.
pub fn schedule_background(task: impl FnOnce() + Send + 'static) -> TaskHandle {
    schedule_async(task)
}

/// Queues `task` for the frame's render phase.
pub fn schedule_render_thread(task: impl FnOnce() + Send + 'static) -> Option<TaskHandle> {
    Some(spawn_task(
        ThreadAffinity::Render,
        TaskPriority::High,
        None,
        None,
        task,
    ))
}

/// Refuses, and says why, rather than running game work on the wrong thread.
pub fn schedule_network_thread<F: FnOnce() + Send + 'static>(_task: F) -> Option<TaskHandle> {
    crate::warn!(
        "the client owns its network thread and posts nothing into it: no task was scheduled"
    );
    None
}

/// The same as [`schedule`], named for the thread it lands on.
pub fn schedule_main_thread(task: impl FnOnce() + Send + 'static) -> Option<TaskHandle> {
    schedule(task)
}

/// The same as [`schedule`]: the game thread is the thread a mod already runs on.
pub fn schedule_game_thread(task: impl FnOnce() + Send + 'static) -> Option<TaskHandle> {
    schedule(task)
}

/// Sends `task` to an executor named as a value.
pub fn dispatch_to(
    executor: TaskExecutor,
    task: impl FnOnce() + Send + 'static,
) -> Option<TaskHandle> {
    executor.submit(task)
}

/// Queues a job and hands back a handle. The same as [`schedule`].
pub fn submit(task: impl FnOnce() + Send + 'static) -> Option<TaskHandle> {
    schedule(task)
}

/// Runs `job` on a worker and hands back a place to put its result.
pub fn submit_with_result<T, F>(job: F) -> TaskResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let slot: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
    let into = Arc::clone(&slot);
    let handle = schedule_async(move || {
        let value = job();
        if let Ok(mut slot) = into.lock() {
            *slot = Some(value);
        }
    });
    TaskResult { handle, slot }
}

/// Takes a task off the queue, or stops a repeating one. False when it had already run and
/// does not repeat.
pub fn cancel(handle: TaskHandle) -> bool {
    if handle.is_none() || handle.is_done() {
        return false;
    }
    let stopped = stop(handle.id);
    if stopped {
        mark(handle.id, TaskStatus::Cancelled);
        count(|metrics| metrics.cancelled += 1);
        TOKENS.with(|tokens| {
            if let Ok(mut tokens) = tokens.lock() {
                if let Some(token) = tokens.remove(&handle.id) {
                    token.cancel();
                }
            }
        });
    }
    stopped
}

fn stop(id: u64) -> bool {
    let mut stopped = false;
    if let Ok(mut queue) = queue().lock() {
        let before = queue.len();
        queue.retain(|task| task.id != id);
        stopped |= queue.len() != before;
    }
    if let Ok(mut queue) = render_queue().lock() {
        let before = queue.len();
        queue.retain(|task| task.id != id);
        stopped |= queue.len() != before;
    }
    if let Ok(mut jobs) = worker_jobs().lock() {
        let before = jobs.len();
        jobs.retain(|task| task.id != id);
        stopped |= jobs.len() != before;
    }
    stopped
}

/// Cancels everything waiting on this mod's queues, and answers how many were stopped.
pub fn cancel_all() -> usize {
    let mut cancelled = 0;
    for queue in [queue(), render_queue(), worker_jobs()] {
        if let Ok(mut queue) = queue.lock() {
            cancelled += queue.len();
            queue.clear();
        }
    }
    count(|metrics| metrics.cancelled += cancelled as u64);
    cancelled
}

/// How a task is doing, by handle.
#[must_use]
pub fn get_status(handle: TaskHandle) -> TaskStatus {
    handle.status()
}

/// Every task that has not run yet, in the order it will run.
#[must_use]
pub fn get_pending() -> Vec<(TaskHandle, Option<Duration>)> {
    let mut pending = Vec::new();
    collect_pending(queue(), &mut pending);
    collect_pending(render_queue(), &mut pending);
    collect_pending(worker_jobs(), &mut pending);
    pending.sort_by_key(|(handle, due)| (due.is_none(), *due, handle.priority().rank()));
    pending
}

fn collect_pending(
    queue: &'static Mutex<Vec<Task>>,
    pending: &mut Vec<(TaskHandle, Option<Duration>)>,
) {
    let now = Instant::now();
    let Ok(queue) = queue.lock() else {
        return;
    };
    for task in queue.iter() {
        pending.push((
            TaskHandle {
                id: task.id,
                affinity: task.affinity,
                priority: task.priority,
            },
            task.due
                .map(|due| due.saturating_duration_since(now)),
        ));
    }
}

/// Waits up to `timeout` for a task to finish, without ever stalling the client: a task
/// queued for the main thread cannot finish while the main thread waits, so this gives up on
/// it immediately and says so.
#[must_use]
pub fn wait_for(handle: TaskHandle, timeout: Duration) -> bool {
    if handle.affinity() != ThreadAffinity::Background && handle.affinity() != ThreadAffinity::Any {
        crate::warn!(
            "task {} is queued for the {}, which is waiting: it can only finish at the next tick",
            handle.id(),
            handle.affinity().as_str()
        );
        return handle.is_done();
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if handle.is_done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    handle.is_done()
}

/// Whether a task has finished, waiting a little for a worker and not at all for this
/// thread. See [`wait_for`] for the rules.
#[must_use]
pub fn wait(handle: TaskHandle) -> bool {
    wait_for(handle, Duration::from_millis(100))
}

/// The same as [`wait`].
#[must_use]
pub fn join(handle: TaskHandle) -> bool {
    wait(handle)
}

/// Records the calling thread as the one a mod runs on. The loader calls this as it starts
/// a mod, so the answers below are about the thread that really runs them.
pub fn record_main_thread() {
    if let Ok(mut threads) = threads().lock() {
        threads.insert("main".to_owned(), std::thread::current().id());
    }
}

/// Whether this is the thread a mod runs on.
#[must_use]
pub fn is_main_thread() -> bool {
    threads()
        .lock()
        .ok()
        .and_then(|threads| threads.get("main").copied())
        .is_some_and(|main| main == current_thread_id())
}

/// Whether this is the thread the game is stepped on, which is the same one here.
#[must_use]
pub fn is_game_thread() -> bool {
    is_main_thread()
}

/// Whether this is the render thread, which is the same thread in a different moment.
#[must_use]
pub fn is_render_thread() -> bool {
    is_main_thread()
}

/// Whether this is the client's network thread. It never is: a mod does not run there.
#[must_use]
pub fn is_network_thread() -> bool {
    false
}

/// Whether this is one of a pool's workers.
#[must_use]
pub fn is_worker_thread() -> bool {
    WORKER_THREADS.with(|threads| {
        threads
            .lock()
            .map(|threads| threads.contains_key(&std::thread::current().id()))
            .unwrap_or(false)
    })
}

/// Checks that this is the main thread, and says what it found.
#[must_use]
pub fn assert_main_thread() -> bool {
    let ok = is_main_thread();
    if !ok {
        crate::warn!(
            "this is {}, which is not the main thread; host calls belong on the main thread",
            current_thread()
        );
    }
    ok
}

/// Checks that this is a moment inside the render phase.
#[must_use]
pub fn assert_render_thread() -> bool {
    let ok = is_render_thread();
    if !ok {
        crate::warn!("render work belongs to the render phase, and this is not it");
    }
    ok
}

/// Checks that this is the thread that steps the world.
#[must_use]
pub fn assert_world_thread() -> bool {
    assert_main_thread()
}

/// A name for the thread this is: `main`, `worker-3`, or the thread's own name.
#[must_use]
pub fn current_thread() -> String {
    if is_main_thread() {
        return "main".to_owned();
    }
    let worker = WORKER_THREADS.with(|threads| {
        threads
            .lock()
            .ok()
            .and_then(|threads| threads.get(&std::thread::current().id()).copied())
    });
    if let Some(id) = worker {
        return format!("worker-{id}");
    }
    std::thread::current()
        .name()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:?}", std::thread::current().id()))
}

/// An id for the thread this is, stable for its lifetime.
#[must_use]
pub fn thread_id() -> u64 {
    let id = current_thread_id();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    id.hash(&mut hasher);
    hasher.finish()
}

fn current_thread_id() -> ThreadId {
    std::thread::current().id()
}

/// How many tasks are waiting.
#[must_use]
pub fn queue_length() -> usize {
    let mut length = 0;
    for queue in [queue(), render_queue(), worker_jobs()] {
        length += queue.lock().map(|queue| queue.len()).unwrap_or_default();
    }
    length
}

/// What the scheduler has been doing, for a mod that wants to see it.
#[must_use]
pub fn get_metrics() -> TaskMetrics {
    let mut metrics = metrics()
        .lock()
        .map(|metrics| *metrics)
        .unwrap_or_default();
    metrics.pending = metrics.scheduled.saturating_sub(
        metrics.ran + metrics.cancelled + metrics.failed,
    ) as usize;
    metrics.queue_length = queue_length();
    metrics
}

/// The loader calls this at the end of every tick, with the tick's length.
pub fn run_due(_dt_millis: u32) -> usize {
    let due = take_due(queue());
    run_all(due)
}

/// The loader calls this as the frame's render phase starts. The render thread is the
/// client's frame thread in this host, so recording it here is the same answer twice.
pub fn run_render_due() -> usize {
    record_main_thread();
    let due = take_due(render_queue());
    run_all(due)
}

fn take_due(queue: &'static Mutex<Vec<Task>>) -> Vec<Task> {
    let Ok(mut tasks) = queue.lock() else {
        return Vec::new();
    };
    let now = Instant::now();
    let mut due = Vec::new();
    let mut waiting = Vec::new();
    for task in tasks.drain(..) {
        if task.due.is_none_or(|due| due <= now) {
            due.push(task);
        } else {
            waiting.push(task);
        }
    }
    *tasks = waiting;
    due
}

fn run_all(mut due: Vec<Task>) -> usize {
    due.sort_by_key(|task| task.priority.rank());
    let ran = due.len();
    for mut task in due {
        if task.cancelled.load(Ordering::SeqCst) {
            mark(task.id, TaskStatus::Cancelled);
            continue;
        }
        let id = task.id;
        let repeating = matches!(task.body, Some(Body::Repeat(_)));
        // A token that says stop ends a repeat before its next run, not after it.
        if repeating && token_stopped(id) {
            mark(id, TaskStatus::Done);
            forget_token(id);
            continue;
        }
        let status = run_body(&mut task);
        if !repeating {
            continue;
        }
        // A repeat goes back for its next turn unless it panicked or a token said stop.
        if status == TaskStatus::Done && !token_stopped(id) {
            task.due = Some(Instant::now() + task.every.unwrap_or_default());
            if let Ok(mut queue) = queue().lock() {
                queue.push(task);
                mark(id, TaskStatus::Scheduled);
            }
        } else {
            forget_token(id);
        }
    }
    ran
}

fn forget_token(id: u64) {
    TOKENS.with(|tokens| {
        if let Ok(mut tokens) = tokens.lock() {
            tokens.remove(&id);
        }
    });
}

fn token_stopped(id: u64) -> bool {
    TOKENS.with(|tokens| {
        tokens
            .lock()
            .ok()
            .and_then(|tokens| tokens.get(&id).map(|token| token.is_cancelled()))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// The queues and the token table are process-wide, so a test that runs a queue takes
    /// the lock: otherwise one test's drain runs another test's task.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn a_scheduled_task_runs_at_the_end_of_the_tick() {
        let _serial = serial();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let handle = submit(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("the main queue is reachable");
        assert_eq!(handle.status(), TaskStatus::Scheduled);
        assert_eq!(ran.load(Ordering::SeqCst), 0, "not yet");
        assert!(run_due(16) >= 1);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(handle.status(), TaskStatus::Done);
        assert!(handle.is_done());
    }

    #[test]
    fn a_task_that_is_not_due_yet_waits() {
        let _serial = serial();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let handle = schedule_after(Duration::from_secs(30), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        run_due(16);
        assert_eq!(ran.load(Ordering::SeqCst), 0, "not due");
        assert_eq!(handle.status(), TaskStatus::Scheduled);
        assert!(cancel(handle));
        assert_eq!(handle.status(), TaskStatus::Cancelled);
    }

    #[test]
    fn a_repeating_task_comes_back() {
        let _serial = serial();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let handle = schedule_repeating(Duration::from_millis(0), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        for _ in 0..3 {
            run_due(16);
        }
        assert!(ran.load(Ordering::SeqCst) >= 3, "it repeats");
        assert!(cancel(handle), "and it can be stopped");
    }

    #[test]
    fn a_background_task_runs_off_this_thread_and_hands_back_a_result() {
        let result: TaskResult<String> = submit_with_result(|| {
            assert!(!is_main_thread(), "a worker is not the main thread");
            format!("computed on {}", current_thread())
        });
        assert!(wait(result.handle()), "a worker finishes promptly");
        let text = result.get().expect("the result is there");
        assert!(text.starts_with("computed on worker-"), "{text}");
        assert!(!result.is_ready(), "the result was taken");
    }

    #[test]
    fn the_network_thread_is_refused_rather_than_faked() {
        assert!(schedule_network_thread(|| {}).is_none());
        assert!(!ThreadAffinity::Network.is_reachable());
        assert!(TaskExecutor::Network.submit(|| {}).is_none());
    }

    #[test]
    fn a_render_task_runs_in_the_render_phase_not_the_tick() {
        let _serial = serial();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        schedule_render_thread(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("the render queue is reachable");
        run_due(16);
        assert_eq!(ran.load(Ordering::SeqCst), 0, "not on the tick");
        assert_eq!(run_render_due(), 1);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn what_is_queued_is_visible_before_it_runs() {
        let _serial = serial();
        let pending_before = get_pending().len();
        let handle = schedule_after(Duration::from_secs(60), || {});
        let pending = get_pending();
        assert!(pending.len() > pending_before);
        assert!(pending.iter().any(|(queued, _)| queued.id() == handle.id()));
        assert_eq!(get_status(handle), TaskStatus::Scheduled);
        cancel(handle);
        assert!(!get_pending().iter().any(|(queued, _)| queued.id() == handle.id()));
    }

    #[test]
    fn metrics_count_what_happened() {
        let _serial = serial();
        let before = get_metrics();
        submit(|| {}).unwrap();
        run_due(16);
        let after = get_metrics();
        assert!(after.scheduled > before.scheduled);
        assert!(after.ran > before.ran);
        assert_eq!(after.queue_length, queue_length());
    }

    #[test]
    fn a_main_thread_task_is_never_waited_for_with_a_stall() {
        let _serial = serial();
        let handle = submit(|| {}).unwrap();
        let started = Instant::now();
        // The task is queued for this thread, which is the one asking, so this must not
        // spin: it answers immediately.
        assert!(!wait(handle));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn a_pool_runs_its_jobs() {
        let pool = WorkerPool::new(2);
        assert_eq!(pool.size(), 2);
        let result: TaskResult<u32> = pool.submit_with_result(|| 6 * 7);
        assert!(wait(result.handle()));
        assert_eq!(result.get(), Some(42));
        let name: TaskResult<String> = pool.submit_with_result(current_thread);
        assert!(wait(name.handle()));
        assert!(
            name.get().expect("the worker named itself").starts_with("worker-"),
            "a pool job runs on a pool worker"
        );
    }

    #[test]
    fn a_group_cancels_together() {
        let _serial = serial();
        let group = JobGroup::new();
        for _ in 0..3 {
            group.add(schedule_after(Duration::from_secs(60), || {}));
        }
        assert_eq!(group.handles().len(), 3);
        assert_eq!(group.cancel(), 3);
        assert!(group.is_finished());
    }

    #[test]
    fn a_token_stops_a_repeating_task() {
        let _serial = serial();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let handle = schedule_repeating(Duration::from_millis(0), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let token = CancellationToken::from_handle(handle);
        run_due(16);
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        token.cancel();
        run_due(16);
        assert_eq!(
            ran.load(Ordering::SeqCst),
            1,
            "a cancelled token stops the repeat before it runs again"
        );
        assert!(token.is_cancelled());
    }

    #[test]
    fn the_thread_a_mod_runs_on_is_the_main_thread() {
        let _serial = serial();
        threads()
            .lock()
            .unwrap()
            .insert("main".to_owned(), current_thread_id());
        assert!(is_main_thread());
        assert!(is_game_thread());
        assert!(is_render_thread());
        assert!(!is_network_thread());
        assert!(assert_main_thread());
        assert!(assert_world_thread());
        assert_eq!(current_thread(), "main");
        assert_ne!(thread_id(), 0);
    }
}
