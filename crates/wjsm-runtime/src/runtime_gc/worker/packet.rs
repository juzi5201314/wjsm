use parking_lot::Mutex;

/// worker packet 的固定 payload 类别；不保存临时对象集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum GcPacketKind {
    PageRange,
    BitmapWordRange,
    RootRange,
    RelocationRange,
}

/// 可在线程间移动的定长 GC work 描述。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GcWorkPacket {
    kind: GcPacketKind,
    start: u64,
    len: u32,
    epoch: u64,
}

impl GcWorkPacket {
    pub const fn new(kind: GcPacketKind, start: u64, len: u32, epoch: u64) -> Self {
        Self {
            kind,
            start,
            len,
            epoch,
        }
    }

    pub const fn kind(self) -> GcPacketKind {
        self.kind
    }

    pub const fn start(self) -> u64 {
        self.start
    }

    pub const fn len(self) -> u32 {
        self.len
    }

    pub const fn epoch(self) -> u64 {
        self.epoch
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PacketId(u32);

impl PacketId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// 固定容量 slab；slot 在处理完成后返回 free list，运行期不增长。
pub(crate) struct PacketSlab {
    state: Mutex<PacketSlabState>,
    capacity: usize,
}

impl PacketSlab {
    pub(crate) fn new(capacity: usize) -> Self {
        let slots = vec![None; capacity];
        let free = (0..capacity)
            .rev()
            .map(|index| PacketId(index as u32))
            .collect();
        Self {
            state: Mutex::new(PacketSlabState { slots, free }),
            capacity,
        }
    }

    pub(crate) fn acquire(&self, packet: GcWorkPacket) -> Option<PacketId> {
        let mut state = self.state.lock();
        let id = state.free.pop()?;
        state.slots[id.index()] = Some(packet);
        Some(id)
    }

    pub(crate) fn take(&self, id: PacketId) -> GcWorkPacket {
        self.state.lock().slots[id.index()]
            .take()
            .expect("queued packet is missing from slab")
    }

    pub(crate) fn release(&self, id: PacketId) {
        self.state.lock().free.push(id);
    }

    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }
}

struct PacketSlabState {
    slots: Vec<Option<GcWorkPacket>>,
    free: Vec<PacketId>,
}
