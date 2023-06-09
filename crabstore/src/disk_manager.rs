use std::{
    fs::*,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(target_os = "linux")]
use std::os::unix::prelude::FileExt;

#[cfg(target_os = "windows")]
use std::os::windows::prelude::FileExt;

use parking_lot::Mutex;

use crate::PAGE_SIZE;
#[derive(Debug)]
pub struct DiskManager {
    file: Mutex<File>,
    next_free_page: AtomicUsize,
}

impl DiskManager {
    pub fn new(file_path: &Path) -> Result<Self, io::Error> {
        Ok(DiskManager {
            file: Mutex::new(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .open(file_path)?,
            ),
            next_free_page: 1.into(),
        })
    }

    pub fn flush(&self) {
        let mut file = self.file.lock();
        file.flush().expect("Failed to flush file to disk");
    }

    #[cfg(target_os = "windows")]
    pub fn read_page(&self, page_id: usize, page: &mut [u8; PAGE_SIZE]) -> usize {
        let file = self.file.lock();
        file.seek_read(page, (page_id * PAGE_SIZE) as u64)
            .expect("Failed to read page")
    }

    #[cfg(target_os = "linux")]
    pub fn read_page(&self, page_id: usize, page: &mut [u8; PAGE_SIZE]) -> usize {
        let file = self.file.lock();
        file.read_at(page, (page_id * PAGE_SIZE) as u64)
            .expect("Failed to read page")
    }

    #[cfg(target_os = "windows")]
    pub fn write_page(&self, page_id: usize, page: &[u8; PAGE_SIZE]) -> usize {
        let file = self.file.lock();
        file.seek_write(page, (page_id * PAGE_SIZE) as u64)
            .expect("Failed to write page")
    }

    #[cfg(target_os = "linux")]
    pub fn write_page(&self, page_id: usize, page: &[u8; PAGE_SIZE]) -> usize {
        let file = self.file.lock();
        file.write_at(page, (page_id * PAGE_SIZE) as u64)
            .expect("Failed to write page")
    }

    pub fn reserve_page(&self) -> usize {
        self.next_free_page.fetch_add(1, Ordering::Relaxed)
    }

    pub fn reserve_range(&self, pages: usize) -> usize {
        self.next_free_page.fetch_add(pages, Ordering::Relaxed)
    }

    pub fn free_page_pointer(&self) -> usize {
        self.next_free_page.load(Ordering::Relaxed)
    }

    pub fn set_free_page_pointer(&self, ptr: usize) {
        self.next_free_page.store(ptr, Ordering::Relaxed)
    }
}
