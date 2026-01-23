#![cfg_attr(target_os = "none", no_std)]
#![doc = include_str!("../README.md")]

extern crate alloc;

use core::{alloc::Layout, num::NonZeroUsize, ops::Deref, ptr::NonNull};

mod osal;

mod array;
mod common;
mod dbox;
// mod slice;

pub use array::*;
pub use common::SingleMapping;
pub use dbox::*;
pub use osal::DmaOp;
// pub use slice::*;

// mod stream;

// pub use stream::*;

/// DMA 传输方向
///
/// 参考 Linux `enum dma_data_direction`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum Direction {
    /// 数据从 CPU 传输到设备 (DMA_TO_DEVICE)
    ToDevice,
    /// 数据从设备传输到 CPU (DMA_FROM_DEVICE)
    FromDevice,
    /// 双向传输 (DMA_BIDIRECTIONAL)
    Bidirectional,
}

/// DMA 地址类型
pub type DmaAddr = u64;

/// 物理地址类型
pub type PhysAddr = u64;

/// DMA 错误类型
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    #[error("DMA allocation failed")]
    NoMemory,
    #[error("Invalid layout for DMA allocation")]
    LayoutError,
    #[error("DMA address {addr:#x} does not match device mask {mask:#x}")]
    DmaMaskNotMatch { addr: DmaAddr, mask: u64 },
}

impl From<core::alloc::LayoutError> for DmaError {
    fn from(_: core::alloc::LayoutError) -> Self {
        DmaError::LayoutError
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DmaHandle {
    pub origin_virt: NonNull<u8>,
    pub dma_addr: DmaAddr,
    pub layout: Layout,
    pub alloc_virt: Option<NonNull<u8>>,
}
impl DmaHandle {
    pub fn size(&self) -> usize {
        self.layout.size()
    }

    pub fn align(&self) -> usize {
        self.layout.align()
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.origin_virt.as_ptr()
    }

    pub(crate) fn dma_virt(&self) -> NonNull<u8> {
        if let Some(virt) = self.alloc_virt {
            virt
        } else {
            self.origin_virt
        }
    }

    pub fn dma_addr(&self) -> DmaAddr {
        self.dma_addr
    }
}
unsafe impl Send for DmaHandle {}

impl Deref for DmaHandle {
    type Target = core::alloc::Layout;
    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

#[derive(Clone)]
pub struct DeviceDma {
    os: &'static dyn DmaOp,
    mask: u64,
}

impl DeviceDma {
    pub fn new(dma_mask: u64, osal: &'static dyn DmaOp) -> Self {
        Self {
            mask: dma_mask,
            os: osal,
        }
    }

    pub fn dma_mask(&self) -> u64 {
        self.mask
    }

    pub fn flush(&self, addr: NonNull<u8>, size: usize) {
        self.os.flush(addr, size)
    }

    pub fn invalidate(&self, addr: NonNull<u8>, size: usize) {
        self.os.invalidate(addr, size)
    }

    pub fn page_size(&self) -> usize {
        self.os.page_size()
    }

    fn prepare_read(&self, handle: &DmaHandle, offset: usize, size: usize, direction: Direction) {
        self.os.prepare_read(handle, offset, size, direction)
    }

    fn confirm_write(&self, handle: &DmaHandle, offset: usize, size: usize, direction: Direction) {
        self.os.confirm_write(handle, offset, size, direction)
    }

    unsafe fn alloc_coherent(&self, layout: core::alloc::Layout) -> Option<DmaHandle> {
        let res = unsafe { self.os.alloc_coherent(self.mask, layout) };
        #[cfg(debug_assertions)]
        {
            if let Some(ref handle) = res {
                assert!(
                    self.mask >= handle.dma_addr + layout.size() as u64,
                    "DMA mask not match: addr={:#x}, size={:#x}, mask={:#x}",
                    handle.dma_addr,
                    layout.size(),
                    self.mask
                );
            }
        }
        res
    }

    unsafe fn dealloc_coherent(&self, handle: DmaHandle) {
        unsafe { self.os.dealloc_coherent(handle) }
    }

    unsafe fn _map_single(
        &self,
        addr: NonNull<u8>,
        size: NonZeroUsize,
        align: usize,
        direction: Direction,
    ) -> Result<DmaHandle, DmaError> {
        let res = unsafe { self.os.map_single(self.mask, addr, size, align, direction) };
        #[cfg(debug_assertions)]
        {
            if let Ok(ref handle) = res {
                assert!(
                    self.mask >= handle.dma_addr + size.get() as u64,
                    "DMA mask not match: addr={:#x}, size={:#x}, mask={:#x}",
                    handle.dma_addr,
                    size,
                    self.mask
                );

                assert!(
                    handle.dma_addr % (align as u64) == 0,
                    "DMA address not aligned: addr={:#x}, align={:#x}",
                    handle.dma_addr,
                    align
                );
            }
        }

        res
    }

    unsafe fn unmap_single(&self, handle: DmaHandle) {
        unsafe { self.os.unmap_single(handle) }
    }

    pub fn new_array<T>(
        &self,
        size: usize,
        align: usize,
        direction: Direction,
    ) -> Result<array::DArray<T>, DmaError> {
        array::DArray::new_zero(self, size, align, direction)
    }

    pub fn new_box<T>(
        &self,
        align: usize,
        direction: Direction,
    ) -> Result<dbox::DBox<T>, DmaError> {
        dbox::DBox::new_zero(self, align, direction)
    }

    pub fn map_single(
        &self,
        addr: NonNull<u8>,
        size: NonZeroUsize,
        align: usize,
        direction: Direction,
    ) -> Result<common::SingleMapping, DmaError> {
        common::SingleMapping::new(self, addr, size, align, direction)
    }
}
