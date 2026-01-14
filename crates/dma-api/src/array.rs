use core::ops::Index;

use alloc::sync::Arc;

use crate::{Direction, DmaError, Osal, common::DCommon};

pub struct DArray<T> {
    data: DCommon<T>,
}

impl<T> DArray<T> {
    pub(crate) fn new_zero(
        os: &Arc<dyn Osal>,
        size: usize,
        align: usize,
        direction: Direction,
    ) -> Result<Self, DmaError> {
        let mut data = DCommon::new(os, size * core::mem::size_of::<T>(), align, direction)?;
        data.as_mut_slice().fill(0);
        data.confirm_write_all();
        Ok(Self { data })
    }

    pub fn dma_addr(&self) -> crate::DmaAddr {
        self.data.handle.dma_addr
    }

    pub fn len(&self) -> usize {
        self.data.handle.size() / core::mem::size_of::<T>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_slice(&self) -> &[T] {
        self.data.prepare_read_all();
        unsafe {
            core::slice::from_raw_parts(self.data.handle.virt_addr.as_ptr() as *const T, self.len())
        }
    }

    pub fn read(&self, index: usize) -> Option<T> {
        if index >= self.len() {
            return None;
        }

        unsafe {
            let ptr = self.data.handle.virt_addr.cast::<T>().add(index);
            self.data.prepare_read(ptr.cast(), size_of::<T>());
            Some(ptr.read_volatile())
        }
    }

    pub fn set(&mut self, index: usize, value: T) {
        assert!(
            index < self.len(),
            "index out of range, index: {},len: {}",
            index,
            self.len()
        );

        unsafe {
            let ptr = self.data.handle.virt_addr.cast::<T>().add(index);
            ptr.write_volatile(value);
            self.data.confirm_write(ptr.cast(), size_of::<T>());
        }
    }
}

impl<T> Index<usize> for DArray<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        unsafe {
            let ptr = self.data.handle.virt_addr.cast::<T>().add(index);
            self.data.prepare_read(ptr.cast(), size_of::<T>());
            &*ptr.as_ptr()
        }
    }
}
