#![cfg_attr(not(any(windows, unix)), no_std)]
#![doc = include_str!("../README.md")]

extern crate alloc;

use core::{ops::Deref, ptr::NonNull};

use alloc::sync::Arc;

mod osal;

mod array;
mod common;
mod dbox;

pub use array::*;
pub use dbox::*;
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
}

impl From<core::alloc::LayoutError> for DmaError {
    fn from(_: core::alloc::LayoutError) -> Self {
        DmaError::LayoutError
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DmaHandle {
    pub virt_addr: NonNull<u8>,
    pub dma_addr: DmaAddr,
    pub layout: core::alloc::Layout,
}

impl Deref for DmaHandle {
    type Target = core::alloc::Layout;
    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

/// 操作系统抽象层 trait
///
/// 用于适配不同的 OS/平台
pub trait Osal: Sync + Send + 'static {
    fn page_size(&self) -> usize;

    /// 将虚拟地址映射到 DMA 地址
    /// 若返回的size小于请求的size，则需要分多次映射
    fn map(&self, addr: NonNull<u8>, size: usize, direction: Direction) -> DmaHandle;

    /// 解除 DMA 映射
    fn unmap(&self, handle: DmaHandle);

    /// 写回缓存到内存 (clean)
    fn flush(&self, addr: NonNull<u8>, size: usize) {
        osal::arch::flush(addr, size)
    }

    /// 使缓存无效 (invalidate)
    fn invalidate(&self, addr: NonNull<u8>, size: usize) {
        osal::arch::invalidate(addr, size)
    }

    /// 分配 DMA 可访问内存
    /// # Safety
    ///
    /// - 调用者必须确保 layout 合法
    /// - 返回的内存必须保证连续
    unsafe fn alloc_coherent(&self, layout: core::alloc::Layout) -> Option<DmaHandle>;

    /// 释放 DMA 内存
    /// # Safety
    /// 调用者必须确保 ptr 和 layout 与 alloc 时匹配
    unsafe fn dealloc_coherent(&self, handle: DmaHandle);

    fn prepare_read(&self, ptr: NonNull<u8>, size: usize, direction: Direction) {
        if matches!(direction, Direction::FromDevice | Direction::Bidirectional) {
            self.invalidate(ptr, size);
        }
    }

    fn confirm_write(&self, ptr: NonNull<u8>, size: usize, direction: Direction) {
        if matches!(direction, Direction::ToDevice | Direction::Bidirectional) {
            self.flush(ptr, size)
        }
    }
}

#[derive(Clone)]
pub struct DmaApi {
    osal: Arc<dyn Osal>,
}

impl DmaApi {
    pub fn new(osal: impl Osal) -> Self {
        Self {
            osal: Arc::new(osal),
        }
    }

    pub fn osal(&self) -> &Arc<dyn Osal> {
        &self.osal
    }

    pub fn new_array<T>(
        &self,
        size: usize,
        align: usize,
        direction: Direction,
    ) -> Result<array::DArray<T>, DmaError> {
        array::DArray::new_zero(&self.osal, size, align, direction)
    }

    pub fn new_box<T>(
        &self,
        align: usize,
        direction: Direction,
    ) -> Result<dbox::DBox<T>, DmaError> {
        dbox::DBox::new_zero(&self.osal, align, direction)
    }
}
