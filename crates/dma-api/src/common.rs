use core::{alloc::Layout, ptr::NonNull};

use alloc::sync::Arc;

use crate::{Direction, DmaError, DmaHandle, Osal};

pub struct DCommon<T> {
    pub handle: DmaHandle,
    pub osal: Arc<dyn Osal>,
    pub direction: Direction,
    _phantom: core::marker::PhantomData<T>,
}

unsafe impl<T: Send> Send for DCommon<T> {}

impl<T> DCommon<T> {
    pub fn new(
        os: &Arc<dyn Osal>,
        size: usize,
        align: usize,
        direction: Direction,
    ) -> Result<Self, DmaError> {
        let layout = Layout::from_size_align(size, align)?;
        let handle = unsafe { os.alloc_coherent(layout) }.ok_or(DmaError::NoMemory)?;
        Ok(Self {
            handle,
            osal: os.clone(),
            direction,
            _phantom: core::marker::PhantomData,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.handle.virt_addr.as_ptr(), self.handle.size()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(self.handle.virt_addr.as_ptr(), self.handle.size())
        }
    }

    pub fn prepare_read(&self, ptr: NonNull<u8>, size: usize) {
        self.osal.prepare_read(ptr, size, self.direction);
    }

    pub fn prepare_read_all(&self) {
        self.osal
            .prepare_read(self.handle.virt_addr, self.handle.size(), self.direction);
    }

    pub fn confirm_write(&self, ptr: NonNull<u8>, size: usize) {
        self.osal.confirm_write(ptr, size, self.direction);
    }

    pub fn confirm_write_all(&self) {
        self.osal
            .confirm_write(self.handle.virt_addr, self.handle.size(), self.direction);
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
