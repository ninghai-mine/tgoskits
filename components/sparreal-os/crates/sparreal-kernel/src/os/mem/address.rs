use core::ptr::NonNull;

use crate::hal::al;
pub use crate::hal::al::{PhysAddr, VirtAddr};

impl VirtAddr {
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.raw() as _
    }
}

impl<T> From<VirtAddr> for *const T {
    fn from(value: VirtAddr) -> Self {
        value.raw() as _
    }
}

impl<T> From<VirtAddr> for *mut T {
    fn from(value: VirtAddr) -> *mut T {
        value.raw() as _
    }
}

impl<T> From<NonNull<T>> for VirtAddr {
    fn from(value: NonNull<T>) -> Self {
        Self::new(value.as_ptr() as _)
    }
}

impl<T> From<*mut T> for VirtAddr {
    fn from(value: *mut T) -> Self {
        Self::new(value as _)
    }
}

#[macro_export]
macro_rules! pa {
    (val: $val:expr) => {
        PhysAddr::new($val as _)
    };
}

#[macro_export]
macro_rules! va {
    (val: $val:expr) => {
        VirtAddr::new($val as _)
    };
}

impl From<VirtAddr> for PhysAddr {
    fn from(value: VirtAddr) -> Self {
        al::memory::virt_to_phys(value)
    }
}

// impl From<PhysAddr> for VirtAddr {
//     fn from(value: PhysAddr) -> Self {
//         al::memory::phys_to_virt(value)
//     }
// }
