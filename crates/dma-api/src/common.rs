use core::{alloc::Layout, num::NonZeroUsize, ptr::NonNull};

use crate::{DeviceDma, Direction, DmaError, DmaHandle, osal::arch::flush};

pub struct DCommon<T> {
    pub handle: DmaHandle,
    pub osal: DeviceDma,
    pub direction: Direction,
    _phantom: core::marker::PhantomData<T>,
}

unsafe impl<T: Send> Send for DCommon<T> {}

impl<T> DCommon<T> {
    pub fn new(
        os: &DeviceDma,
        size: usize,
        align: usize,
        direction: Direction,
    ) -> Result<Self, DmaError> {
        let layout = Layout::from_size_align(size, align)?;
        let handle = unsafe { os.alloc_coherent(layout) }.ok_or(DmaError::NoMemory)?;
        let dma_mask = os.dma_mask();
        if handle.dma_addr > dma_mask {
            unsafe {
                os.dealloc_coherent(handle);
            }
            return Err(DmaError::DmaMaskNotMatch {
                addr: handle.dma_addr,
                mask: dma_mask,
            });
        }

        unsafe {
            core::ptr::write_bytes(handle.dma_virt().as_ptr(), 0, size);
        }
        flush(handle.dma_virt(), size);

        Ok(Self {
            handle,
            osal: os.clone(),
            direction,
            _phantom: core::marker::PhantomData,
        })
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(self.handle.dma_virt().as_ptr(), self.handle.size())
        }
    }

    pub fn prepare_read(&self, offset: usize, size: usize) {
        self.osal
            .prepare_read(&self.handle, offset, size, self.direction);
    }

    pub fn confirm_write(&self, offset: usize, size: usize) {
        self.osal
            .confirm_write(&self.handle, offset, size, self.direction);
    }

    pub fn get_ptr(&self, offset: usize) -> *mut u8 {
        let ptr = unsafe { self.handle.dma_virt().add(offset) };
        ptr.as_ptr()
    }

    pub fn confirm_write_all(&self) {
        self.osal
            .confirm_write(&self.handle, 0, self.handle.size(), self.direction);
    }
}

impl<T> Drop for DCommon<T> {
    fn drop(&mut self) {
        if self.handle.size() > 0 {
            unsafe {
                self.osal.dealloc_coherent(self.handle);
            }
        }
    }
}

pub struct SingleMapping {
    pub handle: DmaHandle,
    osal: DeviceDma,
    pub direction: Direction,
}

unsafe impl Send for SingleMapping {}

impl SingleMapping {
    pub(crate) fn new(
        os: &DeviceDma,
        addr: NonNull<u8>,
        size: NonZeroUsize,
        align: usize,
        direction: Direction,
    ) -> Result<Self, DmaError> {
        let handle = unsafe { os._map_single(addr, size, align, direction)? };
        let dma_mask = os.dma_mask();
        if handle.dma_addr > dma_mask {
            unsafe {
                os.unmap_single(handle);
            }
            return Err(DmaError::DmaMaskNotMatch {
                addr: handle.dma_addr,
                mask: dma_mask,
            });
        }

        Ok(Self {
            handle,
            osal: os.clone(),
            direction,
        })
    }

    pub fn len(&self) -> usize {
        self.handle.size()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取 DMA 地址
    pub fn dma_addr(&self) -> crate::DmaAddr {
        self.handle.dma_addr
    }

    pub fn prepare_read_all(&self) {
        self.osal
            .prepare_read(&self.handle, 0, self.len(), self.direction);
    }

    pub fn confirm_write_all(&self) {
        self.osal
            .confirm_write(&self.handle, 0, self.len(), self.direction);
    }
}

impl Drop for SingleMapping {
    fn drop(&mut self) {
        self.confirm_write_all();
        self.prepare_read_all();
        unsafe {
            self.osal.unmap_single(self.handle);
        }
    }
}
