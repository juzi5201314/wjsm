use crossbeam_deque::{Injector, Steal, Stealer, Worker};

use super::packet::PacketId;

pub(crate) struct WorkerQueues {
    pub(crate) injector: Injector<PacketId>,
    stealers: Vec<Stealer<PacketId>>,
}

impl WorkerQueues {
    pub(crate) fn new(worker_count: usize) -> (Self, Vec<Worker<PacketId>>) {
        let mut locals = Vec::with_capacity(worker_count);
        let mut stealers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let worker = Worker::new_fifo();
            stealers.push(worker.stealer());
            locals.push(worker);
        }
        (
            Self {
                injector: Injector::new(),
                stealers,
            },
            locals,
        )
    }

    pub(crate) fn pop(&self, local: &Worker<PacketId>, worker_index: usize) -> Option<Dequeued> {
        local.pop().map(Dequeued::local).or_else(|| {
            loop {
                match self.injector.steal_batch_and_pop(local) {
                    Steal::Success(packet) => return Some(Dequeued::local(packet)),
                    Steal::Retry => continue,
                    Steal::Empty => match self.steal_peer(worker_index) {
                        Steal::Success(packet) => return Some(Dequeued::stolen(packet)),
                        Steal::Retry => continue,
                        Steal::Empty => return None,
                    },
                }
            }
        })
    }

    pub(crate) fn injector_is_empty(&self) -> bool {
        self.injector.is_empty()
    }

    fn steal_peer(&self, worker_index: usize) -> Steal<PacketId> {
        self.stealers
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != worker_index)
            .map(|(_, stealer)| stealer.steal())
            .collect()
    }
}

pub(crate) struct Dequeued {
    pub(crate) packet: PacketId,
    pub(crate) stolen: bool,
}

impl Dequeued {
    fn local(packet: PacketId) -> Self {
        Self {
            packet,
            stolen: false,
        }
    }

    fn stolen(packet: PacketId) -> Self {
        Self {
            packet,
            stolen: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerQueues;
    use crate::runtime_gc::worker::packet::PacketSlab;
    use crate::runtime_gc::worker::{GcPacketKind, GcWorkPacket};

    #[test]
    fn worker_steals_packet_from_peer_local_deque() {
        let (queues, mut locals) = WorkerQueues::new(2);
        let owner = locals.remove(0);
        let thief = locals.remove(0);
        let slab = PacketSlab::new(1);
        let packet = GcWorkPacket::new(GcPacketKind::PageRange, 3, 1, 7);
        let id = slab.acquire(packet).unwrap();

        owner.push(id);
        let dequeued = queues.pop(&thief, 1).unwrap();

        assert!(dequeued.stolen);
        assert_eq!(slab.take(dequeued.packet), packet);
    }
}
