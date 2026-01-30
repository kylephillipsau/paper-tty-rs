//! Shared memory buffer pool for Wayland frame data.
//!
//! Creates a POSIX shared memory segment via shm_open, mmaps it, and
//! wraps it in a wl_shm_pool + wl_buffer for screencopy.

use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};
use rustix::shm::{shm_open, Mode as ShmMode};
use rustix::shm::ShmOFlags;
use std::ffi::CStr;
use std::num::NonZeroUsize;
use std::os::fd::AsFd;
use std::ptr::NonNull;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_buffer::WlBuffer;

use crate::error::{Error, Result};

/// A shared-memory buffer backed by POSIX shm + mmap.
pub struct ShmBuffer {
    pool: WlShmPool,
    buffer: WlBuffer,
    ptr: NonNull<u8>,
    size: usize,
}

// SAFETY: The mmap'd memory is only accessed from our thread; the Wayland
// protocol handles synchronization via buffer release events.
unsafe impl Send for ShmBuffer {}

impl ShmBuffer {
    /// Create a new shared memory buffer suitable for XRGB8888 screencopy.
    ///
    /// `width` and `height` are in pixels; stride = width * 4.
    pub fn new(
        shm: &wl_shm::WlShm,
        qh: &wayland_client::QueueHandle<super::screencopy::State>,
        width: u32,
        height: u32,
        format: wl_shm::Format,
    ) -> Result<Self> {
        let stride = width * 4;
        let size = (stride * height) as usize;

        // Create POSIX shared memory
        let name = CStr::from_bytes_with_nul(b"/paper-tty-screencopy\0").unwrap();
        let fd = shm_open(
            name,
            ShmOFlags::CREATE | ShmOFlags::RDWR | ShmOFlags::EXCL,
            ShmMode::RUSR | ShmMode::WUSR,
        )
        .or_else(|_| {
            // Already exists, unlink and retry
            let _ = rustix::shm::shm_unlink(name);
            shm_open(
                name,
                ShmOFlags::CREATE | ShmOFlags::RDWR | ShmOFlags::EXCL,
                ShmMode::RUSR | ShmMode::WUSR,
            )
        })
        .map_err(|e| Error::Wayland(format!("shm_open: {}", e)))?;

        // Unlink immediately so it's cleaned up when fd closes
        let _ = rustix::shm::shm_unlink(name);

        // Set size
        rustix::fs::ftruncate(&fd, size as u64)
            .map_err(|e| Error::Wayland(format!("ftruncate: {}", e)))?;

        // mmap
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                NonZeroUsize::new(size).ok_or_else(|| Error::Wayland("zero size".into()))?.into(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                fd.as_fd(),
                0,
            )
            .map_err(|e| Error::Wayland(format!("mmap: {}", e)))?
        };
        let ptr = NonNull::new(ptr as *mut u8)
            .ok_or_else(|| Error::Wayland("mmap returned null".into()))?;

        // Create wl_shm_pool and wl_buffer
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            qh,
            (),
        );

        Ok(Self {
            pool,
            buffer,
            ptr,
            size,
        })
    }

    /// Get the wl_buffer for passing to screencopy.
    pub fn wl_buffer(&self) -> &WlBuffer {
        &self.buffer
    }

    /// Get a slice of the mapped buffer data (XRGB8888 pixels).
    pub fn data(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }

    /// Get a mutable slice of the mapped buffer data.
    pub fn data_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        unsafe {
            let _ = munmap(self.ptr.as_ptr() as *mut _, self.size);
        }
    }
}
