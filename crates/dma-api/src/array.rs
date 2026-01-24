use core::{alloc::Layout, ops::Index};

use crate::{DeviceDma, DmaDirection, DmaError, common::DCommon};

pub struct DArray<T> {
    data: DCommon,
    _phantom: core::marker::PhantomData<T>,
}

unsafe impl<T> Send for DArray<T> where T: Send {}

impl<T> DArray<T> {
    pub(crate) fn new_zero_with_align(
        os: &DeviceDma,
        size: usize,
        algin: usize,
        direction: DmaDirection,
    ) -> Result<Self, DmaError> {
        let layout = Layout::from_size_align(size, algin.max(core::mem::align_of::<T>()))?;
        let data = DCommon::new_zero(os, layout, direction)?;
        Ok(Self {
            data,
            _phantom: core::marker::PhantomData,
        })
    }

    pub(crate) fn new_zero(
        os: &DeviceDma,
        size: usize,
        direction: DmaDirection,
    ) -> Result<Self, DmaError> {
        Self::new_zero_with_align(os, size, core::mem::align_of::<T>(), direction)
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

    pub fn read(&self, index: usize) -> Option<T> {
        if index >= self.len() {
            return None;
        }

        unsafe {
            let offset = index * core::mem::size_of::<T>();
            self.data.prepare_read(offset, size_of::<T>());
            Some(self.data.dma_ptr(offset).cast::<T>().read_volatile())
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
            let offset = index * size_of::<T>();
            let ptr = self.data.dma_ptr(offset).cast::<T>();
            ptr.write_volatile(value);
            self.data.confirm_write(offset, size_of::<T>());
        }
    }

    pub fn iter(&self) -> DArrayIter<'_, T> {
        DArrayIter {
            array: self,
            index: 0,
        }
    }

    pub fn copy_from_slice(&mut self, src: &[T]) {
        assert!(
            src.len() <= self.len(),
            "source slice is larger than DArray, src len: {}, DArray len: {}",
            src.len(),
            self.len()
        );
        let src_bytes = unsafe {
            core::slice::from_raw_parts(src.as_ptr() as *const u8, core::mem::size_of_val(src))
        };
        self.data.as_mut_slice().copy_from_slice(src_bytes);
        self.data.confirm_write_all();
    }

    /// # Safety
    ///
    /// slice will not auto do cache sync operations.
    pub unsafe fn as_mut_slice(&mut self) -> &mut [T] {
        let byte_slice = self.data.as_mut_slice();
        unsafe {
            core::slice::from_raw_parts_mut(
                byte_slice.as_mut_ptr() as *mut T,
                byte_slice.len() / core::mem::size_of::<T>(),
            )
        }
    }

    pub fn as_ptr(&self) -> *mut T {
        self.data.handle.as_ptr().cast::<T>()
    }
}

pub struct DArrayIter<'a, T> {
    array: &'a DArray<T>,
    index: usize,
}

impl<'a, T> Iterator for DArrayIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.array.len() {
            return None;
        }
        let value = self.array.read(self.index);
        self.index += 1;
        value
    }
}

/// 注意：Index 实现返回引用，调用时会自动执行缓存同步。
/// 但由于返回的是引用，在持有引用期间如果设备继续写入数据，
/// 可能导致数据不一致。对于 `FromDevice` 方向，建议使用 `read()` 方法。
impl<T: Copy> Index<usize> for DArray<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        assert!(
            index < self.len(),
            "index out of range, index: {},len: {}",
            index,
            self.len()
        );
        unsafe {
            let offset = index * core::mem::size_of::<T>();
            let ptr = self.data.dma_ptr(offset).cast::<T>();
            self.data.prepare_read(offset, size_of::<T>());
            &*ptr
        }
    }
}
