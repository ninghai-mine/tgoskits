use crate::{
    FramAllocator, PageTableEntry, PagingError, PagingResult, PhysAddr, TableGeneric, VirtAddr,
};

/// 页表帧，代表一个物理页面上的页表
#[derive(Clone, Copy)]
pub struct Frame<T: TableGeneric, A: FramAllocator> {
    pub paddr: PhysAddr,
    pub allocator: A,
    _marker: core::marker::PhantomData<T>,
}

impl<T: TableGeneric, A: FramAllocator> core::fmt::Debug for Frame<T, A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Frame")
            .field("paddr", &format_args!("{:#x}", self.paddr.raw()))
            .finish()
    }
}

impl<T, A> Frame<T, A>
where
    T: TableGeneric,
    A: FramAllocator,
{
    pub(crate) const PT_INDEX_SHIFT: usize = T::PAGE_SIZE.trailing_zeros() as usize;
    pub(crate) const PT_INDEX_BITS: usize = cal_index_bits::<T>();
    pub(crate) const PT_VALID_BITS: usize = Self::PT_INDEX_BITS + Self::PT_INDEX_SHIFT;
    pub(crate) const LEN: usize = T::PAGE_SIZE / core::mem::size_of::<T::P>();
    pub(crate) const PT_INDEX_MASK: usize = (1 << (Self::PT_VALID_BITS - Self::PT_INDEX_SHIFT)) - 1;
    pub(crate) const PT_LEVEL: usize = T::LEVEL_BITS.len();

    /// 创建新的页表帧（分配并清零）
    pub fn new(allocator: A) -> PagingResult<Self> {
        let paddr = allocator.alloc_frame().ok_or(PagingError::NoMemory)?;
        unsafe {
            let vaddr = allocator.phys_to_virt(paddr);
            core::ptr::write_bytes(vaddr, 0, T::PAGE_SIZE);
        }

        Ok(Self {
            paddr,
            allocator,
            _marker: core::marker::PhantomData,
        })
    }

    /// 从物理地址创建Frame（不分配）
    pub fn from_paddr(paddr: PhysAddr, allocator: A) -> Self {
        Self {
            paddr,
            allocator,
            _marker: core::marker::PhantomData,
        }
    }

    /// 从PTE创建子Frame（用于遍历子页表）
    pub fn from_pte(pte: &T::P, allocator: A) -> Self {
        Self::from_paddr(pte.paddr(), allocator)
    }

    /// 获取页表项的可变切片
    pub fn as_slice_mut(&mut self) -> &mut [T::P] {
        let vaddr = self.allocator.phys_to_virt(self.paddr);
        unsafe { core::slice::from_raw_parts_mut(vaddr as *mut T::P, Self::LEN) }
    }

    /// 获取页表项的不可变切片
    pub fn as_slice(&self) -> &[T::P] {
        let vaddr = self.allocator.phys_to_virt(self.paddr);
        unsafe { core::slice::from_raw_parts(vaddr as *const T::P, Self::LEN) }
    }

    /// 计算指定级别对应的映射大小
    /// - Level 1 (叶子): PAGE_SIZE
    /// - Level 2: PAGE_SIZE << INDEX_BITS
    /// - Level 3: PAGE_SIZE << (INDEX_BITS * 2)
    /// - Level 4: PAGE_SIZE << (INDEX_BITS * 3)
    pub fn level_size(level: usize) -> usize {
        let shift = T::LEVEL_BITS.iter().take(level - 1).sum::<usize>();
        T::PAGE_SIZE << shift
    }

    /// 计算指定级别的页表索引
    /// 从虚拟地址中提取对应级别的索引位
    pub fn virt_to_index(vaddr: VirtAddr, level: usize) -> usize {
        if level == 0 || level > Self::PT_LEVEL {
            panic!("Invalid level: {} (valid: 1..={})", level, Self::PT_LEVEL);
        }
        // Level 1 (叶子): shift = page_shift + 0 * INDEX_BITS (取bits [20:12])
        // Level 2: shift = page_shift + 1 * INDEX_BITS (取bits [29:21])
        // Level 3: shift = page_shift + 2 * INDEX_BITS (取bits [38:30])
        // Level 4 (根): shift = page_shift + 3 * INDEX_BITS (取bits [47:39])
        let page_shift = T::PAGE_SIZE.trailing_zeros() as usize;
        let shift = page_shift + (level - 1) * Self::PT_INDEX_BITS;
        (vaddr.raw() >> shift) & Self::PT_INDEX_MASK
    }

    /// 重建完整的虚拟地址
    /// 从基地址和索引计算完整的虚拟地址
    pub fn reconstruct_vaddr(index: usize, level: usize, base_vaddr: VirtAddr) -> VirtAddr {
        let entry_size = Self::level_size(level);
        base_vaddr + index * entry_size
    }
}

const fn cal_index_bits<T: TableGeneric>() -> usize {
    let mut bits = 0;
    let len = T::LEVEL_BITS.len();
    let mut i = 0;
    while i < len {
        bits += T::LEVEL_BITS[i];
        i += 1;
    }
    bits
}
