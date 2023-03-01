use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{self, Ordering},
        Mutex, RwLock,
    },
};

use nohash::BuildNoHashHasher;
use rclite::Arc;

use crate::{disk_manager::DiskManager, page::PhysicalPage};

pub struct BufferPoolFrame {
    page_id: atomic::AtomicUsize,
    dirty: atomic::AtomicBool,
    page: RwLock<PhysicalPage>,
}

impl BufferPoolFrame {
    pub fn new() -> Self {
        BufferPoolFrame {
            page_id: (!0).into(),
            dirty: false.into(),
            page: RwLock::new(PhysicalPage::default()),
        }
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    pub fn get_page_id(&self) {
        self.page_id.load(Ordering::SeqCst);
    }
}

pub struct BufferPool {
    disk: DiskManager,
    size: usize,
    page_frame_map: HashMap<usize, usize, BuildNoHashHasher<usize>>,
    frames: Vec<Arc<BufferPoolFrame>>,
    clock_refs: Vec<bool>,
    clock_hand: usize,
}

impl BufferPool {
    pub fn new(disk: DiskManager, size: usize) -> Self {
        let mut frames = Vec::with_capacity(size);
        let mut page_frame_map =
            HashMap::with_capacity_and_hasher(size, BuildNoHashHasher::default());
        let mut clock_refs = Vec::with_capacity(size);

        for i in 0..size {
            frames.push(Arc::new(BufferPoolFrame::new()));
            clock_refs.push(false);
        }

        BufferPool {
            disk,
            size,
            page_frame_map,
            frames,
            clock_refs,
            clock_hand: 0,
        }
    }

    fn find_evict_victim(&self) -> usize {
        let victim = loop {
            if self.clock_refs[self.clock_hand] || self.frames[self.clock_hand].strong_count() > 1 {
                self.clock_refs[self.clock_hand] = false;
                self.clock_hand = (self.clock_hand + 1) % self.size;
                continue;
            }

            break self.clock_hand;
        };

        self.clock_hand = (self.clock_hand + 1) % self.size;

        victim
    }

    fn evict(&self, victim: usize) {
        self.page_frame_map.remove(&victim);

        let frame = &self.frames[self.clock_hand];

        frame.page_id.store(!0, Ordering::SeqCst);

        if frame.dirty.load(Ordering::SeqCst) {
            let page = frame
                .page
                .write()
                .expect("Failed to acquire lock, lock poisoning?");

            self.disk
                .write_page(frame.page_id.load(Ordering::SeqCst), &page.page);

            self.disk.flush();

            frame.dirty.store(false, Ordering::SeqCst);
        }
    }

    pub fn new_page(&mut self) -> Arc<BufferPoolFrame> {
        let new_page_id = self.disk.reserve_page();

        let victim = self.find_evict_victim();

        self.evict(victim);

        let frame = self.frames[victim];

        frame.page_id.store(new_page_id, Ordering::SeqCst);
        self.page_frame_map.insert(new_page_id, victim);

        Arc::clone(&frame)
    }

    pub fn get_page(&mut self, page_id: usize) -> Arc<BufferPoolFrame> {
        if let Some(frame_id) = self.page_frame_map.get(&page_id) {
            self.clock_refs[*frame_id] = true;
            return Arc::clone(&self.frames[*frame_id]);
        }

        let victim = self.find_evict_victim();

        self.evict(victim);

        let frame = self.frames[victim];

        frame.page_id.store(page_id, Ordering::SeqCst);

        self.clock_refs[victim] = true;

        let page = frame
            .page
            .write()
            .expect("Failed to acquire RwLock, poisoned?");

        self.disk.read_page(page_id, &mut page.page);

        drop(page);

        self.page_frame_map.insert(page_id, victim);

        Arc::clone(&frame)
    }
}
