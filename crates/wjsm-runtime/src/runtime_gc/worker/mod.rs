mod packet;
mod queue;

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use parking_lot::{Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crossbeam_deque::Worker;

pub use packet::{GcPacketKind, GcWorkPacket};

use packet::PacketSlab;
use queue::WorkerQueues;
use crate::heap::platform::{NumaTopology, set_thread_affinity};

/// 固定 worker pool 在关闭和容量边界上返回的显式错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerPoolError {
    InvalidPacketCapacity,
    InvalidWorkerCount,
    PacketSlabExhausted,
    ShuttingDown,
    ThreadSpawn(String),
}

impl fmt::Display for WorkerPoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPacketCapacity => {
                formatter.write_str("packet slab capacity must fit u32 and be nonzero")
            }
            Self::InvalidWorkerCount => formatter.write_str("GC worker count must be nonzero"),
            Self::PacketSlabExhausted => formatter.write_str("GC packet slab is exhausted"),
            Self::ShuttingDown => formatter.write_str("GC worker pool is shutting down"),
            Self::ThreadSpawn(error) => write!(formatter, "unable to spawn GC worker: {error}"),
        }
    }
}

impl Error for WorkerPoolError {}

/// worker 生命周期和调度测量；所有值为自 pool 创建以来的累积值。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkerPoolStats {
    pub submitted: u64,
    pub completed: u64,
    pub steals: u64,
    pub parks: u64,
}

type PacketHandler = dyn Fn(usize, GcWorkPacket) + Send + Sync + 'static;

/// Store-free、固定大小的 V2 GC worker pool。
pub struct GcWorkerPool {
    shared: Arc<SharedPool>,
    joins: Mutex<Vec<JoinHandle<()>>>,
}

impl GcWorkerPool {
    pub fn new<F>(
        worker_count: usize,
        packet_capacity: usize,
        handler: F,
    ) -> Result<Self, WorkerPoolError>
    where
        F: Fn(usize, GcWorkPacket) + Send + Sync + 'static,
    {
        if worker_count == 0 {
            return Err(WorkerPoolError::InvalidWorkerCount);
        }
        if packet_capacity == 0 || packet_capacity > u32::MAX as usize {
            return Err(WorkerPoolError::InvalidPacketCapacity);
        }
        let (queues, locals) = WorkerQueues::new(worker_count);
        let shared = Arc::new(SharedPool::new(queues, packet_capacity, Arc::new(handler)));
        let mut joins = Vec::with_capacity(worker_count);
        for (worker_index, local) in locals.into_iter().enumerate() {
            shared.live_workers.fetch_add(1, Ordering::SeqCst);
            let worker_shared = Arc::clone(&shared);
            let builder = thread::Builder::new().name(format!("wjsm-gc-{worker_index}"));
            // Soft NUMA affinity: prefer current topology node; portable no-op off Linux.
            let topology = NumaTopology::detect();
            let node = topology.current_node;
            match builder.spawn(move || {
                let _ = set_thread_affinity(node);
                worker_loop(worker_index, local, worker_shared)
            }) {
                Ok(join) => joins.push(join),
                Err(error) => {
                    shared.live_workers.fetch_sub(1, Ordering::SeqCst);
                    shared.begin_shutdown();
                    for join in joins {
                        join.join()
                            .expect("GC worker panicked during startup shutdown");
                    }
                    return Err(WorkerPoolError::ThreadSpawn(error.to_string()));
                }
            }
        }
        Ok(Self {
            shared,
            joins: Mutex::new(joins),
        })
    }

    pub fn submit(&self, packet: GcWorkPacket) -> Result<(), WorkerPoolError> {
        if !self.shared.accepting.load(Ordering::SeqCst) {
            return Err(WorkerPoolError::ShuttingDown);
        }
        let id = self
            .shared
            .slab
            .acquire(packet)
            .ok_or(WorkerPoolError::PacketSlabExhausted)?;
        if !self.shared.accepting.load(Ordering::SeqCst) {
            self.shared.slab.release(id);
            return Err(WorkerPoolError::ShuttingDown);
        }
        self.shared.inflight.fetch_add(1, Ordering::SeqCst);
        self.shared.submitted.fetch_add(1, Ordering::Relaxed);
        self.shared.queues.injector.push(id);
        self.shared.notify_all();
        Ok(())
    }

    pub fn wait_for_idle(&self) {
        let (mutex, condvar) = &self.shared.parking;
        let mut state = mutex.lock();
        while self.shared.inflight.load(Ordering::SeqCst) != 0 {
            condvar.wait(&mut state);
        }
    }

    pub fn wait_for_parked_workers(&self, expected: usize) {
        let (mutex, condvar) = &self.shared.parking;
        let mut state = mutex.lock();
        while state.parked < expected {
            condvar.wait(&mut state);
        }
    }

    pub fn shutdown(&self) {
        self.shared.begin_shutdown();
        let joins = std::mem::take(&mut *self.joins.lock());
        for join in joins {
            join.join().expect("GC worker panicked during shutdown");
        }
        self.wait_for_stopped();
    }

    pub fn packet_slab_allocations(&self) -> usize {
        self.shared.slab.capacity()
    }

    pub fn live_workers(&self) -> usize {
        self.shared.live_workers.load(Ordering::SeqCst)
    }

    pub fn stats(&self) -> WorkerPoolStats {
        WorkerPoolStats {
            submitted: self.shared.submitted.load(Ordering::Relaxed),
            completed: self.shared.completed.load(Ordering::Relaxed),
            steals: self.shared.steals.load(Ordering::Relaxed),
            parks: self.shared.parks.load(Ordering::Relaxed),
        }
    }

    fn wait_for_stopped(&self) {
        let (mutex, condvar) = &self.shared.parking;
        let mut state = mutex.lock();
        while self.shared.live_workers.load(Ordering::SeqCst) != 0 {
            condvar.wait(&mut state);
        }
    }
}

impl Drop for GcWorkerPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct SharedPool {
    queues: WorkerQueues,
    slab: PacketSlab,
    handler: Arc<PacketHandler>,
    accepting: AtomicBool,
    shutdown: AtomicBool,
    inflight: AtomicUsize,
    live_workers: AtomicUsize,
    submitted: AtomicU64,
    completed: AtomicU64,
    steals: AtomicU64,
    parks: AtomicU64,
    parking: (Mutex<ParkingState>, Condvar),
}

impl SharedPool {
    fn new(queues: WorkerQueues, packet_capacity: usize, handler: Arc<PacketHandler>) -> Self {
        Self {
            queues,
            slab: PacketSlab::new(packet_capacity),
            handler,
            accepting: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            inflight: AtomicUsize::new(0),
            live_workers: AtomicUsize::new(0),
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            steals: AtomicU64::new(0),
            parks: AtomicU64::new(0),
            parking: (Mutex::new(ParkingState::default()), Condvar::new()),
        }
    }

    fn begin_shutdown(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.shutdown.store(true, Ordering::SeqCst);
        self.notify_all();
    }

    fn should_exit(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst) && self.inflight.load(Ordering::SeqCst) == 0
    }

    fn notify_all(&self) {
        self.parking.1.notify_all();
    }
}

#[derive(Default)]
struct ParkingState {
    parked: usize,
}

fn worker_loop(worker_index: usize, local: Worker<packet::PacketId>, shared: Arc<SharedPool>) {
    loop {
        if let Some(dequeued) = shared.queues.pop(&local, worker_index) {
            process_packet(worker_index, dequeued, &shared);
            continue;
        }
        if shared.should_exit() {
            break;
        }
        park_until_work_or_shutdown(&shared);
    }
    shared.live_workers.fetch_sub(1, Ordering::SeqCst);
    shared.notify_all();
}

fn process_packet(worker_index: usize, dequeued: queue::Dequeued, shared: &SharedPool) {
    if dequeued.stolen {
        shared.steals.fetch_add(1, Ordering::Relaxed);
    }
    let packet = shared.slab.take(dequeued.packet);
    (shared.handler)(worker_index, packet);
    shared.slab.release(dequeued.packet);
    let previous = shared.inflight.fetch_sub(1, Ordering::SeqCst);
    debug_assert!(previous > 0);
    shared.completed.fetch_add(1, Ordering::Relaxed);
    shared.notify_all();
}

fn park_until_work_or_shutdown(shared: &SharedPool) {
    let (mutex, condvar) = &shared.parking;
    let mut state = mutex.lock();
    if shared.should_exit() || !shared.queues.injector_is_empty() {
        return;
    }
    state.parked += 1;
    shared.parks.fetch_add(1, Ordering::Relaxed);
    condvar.notify_all();
    while !shared.should_exit() && shared.queues.injector_is_empty() {
        condvar.wait(&mut state);
    }
    state.parked -= 1;
    condvar.notify_all();
}
