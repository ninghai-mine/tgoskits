use crate::{DeviceDma, Direction, DmaError, common::DCommon};

pub struct DBox<T> {
    data: DCommon<T>,
}

unsafe impl<T> Send for DBox<T> where T: Send {}

impl<T> DBox<T> {
    pub(crate) fn new_zero(
        os: &DeviceDma,
        align: usize,
        direction: Direction,
    ) -> Result<Self, DmaError> {
        let data = DCommon::new(os, core::mem::size_of::<T>(), align, direction)?;
        Ok(Self { data })
    }

    pub fn dma_addr(&self) -> crate::DmaAddr {
        self.data.handle.dma_addr
    }

    pub fn read(&self) -> T {
        unsafe {
            self.data.prepare_read(0, core::mem::size_of::<T>());
            let ptr = self.data.get_ptr(0).cast::<T>();
            ptr.read_volatile()
        }
    }

    pub fn write(&mut self, value: T) {
        unsafe {
            let ptr = self.data.get_ptr(0).cast::<T>();
            ptr.write_volatile(value);
            self.data.confirm_write(0, core::mem::size_of::<T>());
        }
    }

    pub fn modify(&mut self, f: impl FnOnce(&mut T)) {
        let mut value = self.read();
        f(&mut value);
        self.write(value);
    }

    /// 获取底层缓冲区的可变切片
    ///
    /// # Safety
    ///
    /// - 调用者必须确保在使用该切片期间，设备不会访问此内存区域
    /// - 调用者必须手动处理缓存同步（flush/invalidate）
    pub unsafe fn as_buff_slice_mut(&mut self) -> &mut [u8] {
        self.data.as_mut_slice()
    }
}
