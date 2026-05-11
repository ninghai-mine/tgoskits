use core::ptr::slice_from_raw_parts;

use crate::TestCase;

#[repr(C)]
pub struct ListRef {
    data: *const u8,
    len: usize,
}

impl ListRef {
    pub fn from_raw(data: &'static [u8]) -> Self {
        Self {
            data: data.as_ptr() as _,
            len: data.len(),
        }
    }

    pub fn iter(&self) -> Iter<'static> {
        Iter::new(unsafe { core::slice::from_raw_parts(self.data, self.len) })
    }
}

pub struct Iter<'a> {
    data: &'a [u8],
    iter: usize,
}

impl<'a> Iter<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Iter { data, iter: 0 }
    }
}

impl Iterator for Iter<'_> {
    type Item = TestCase;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }

        let slice = unsafe {
            &*slice_from_raw_parts(
                self.data.as_ptr() as *const TestCase,
                self.data.len() / size_of::<TestCase>(),
            )
        };
        let out = slice.get(self.iter).cloned();
        self.iter += 1;
        out
    }
}
