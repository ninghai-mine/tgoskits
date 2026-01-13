//! LoongArch64 页表项 (Page Table Entry)
//!
//! 使用 tock-registers 风格定义页表项，提供类型安全的寄存器访问
//! 参考: LoongArch64 参考手册 Vol. 1 - 5.4.2 节

use core::fmt::Debug;

use loongArch64::register::asid;
use page_table_generic::{MemAttributes, PageTableEntry};
use tock_registers::interfaces::*;
use tock_registers::register_bitfields;
use tock_registers::registers::*;

// LoongArch64 页表项寄存器位域定义
register_bitfields![u64,
    /// LoongArch64 单页页表项 (Page Table Entry)
    ///
    /// 布局参考 LoongArch64 参考手册 5.4.2 节
    PTE_DIR [
        /// V - 有效位 (bit 0)
        VALID OFFSET(0) NUMBITS(1) [],

        /// D - 脏位 (bit 1)
        DIRTY OFFSET(1) NUMBITS(1) [],

        /// PLV - 特权级 (bits 2-3)
        PLV OFFSET(2) NUMBITS(2) [
            PLV0 = 0b00,  // 内核态
            PLV1 = 0b01,  // 特权级1
            PLV2 = 0b10,  // 特权级2
            PLV3 = 0b11   // 用户态
        ],

        /// 缓存属性 (bits 4-5)
        CACHE OFFSET(4) NUMBITS(2) [
            SUC = 0b00,  // 强序非缓存 (Strongly-ordered UnCached)
            CC  = 0b01,  // 一致性缓存 (Coherent Cached)
            WUC = 0b10   // 弱序非缓存 (Weakly-ordered UnCached)
        ],

        /// H/G - 共享位（bit 6）
        /// 在目录项中：H=1 表示大页映射
        /// 在页表项中：G=1 表示全局映射（此时 H 必须为 0）
        /// 注意：根据上下文区分是 H 位还是 G 位，不能同时为 1
        H OFFSET(6) NUMBITS(1) [],

        /// P - 存在位 (bit 7)
        PRESENT OFFSET(7) NUMBITS(1) [],

        /// W - 写位 (bit 8)
        WRITE OFFSET(8) NUMBITS(1) [],

        G OFFSET(12) NUMBITS(1) [],

        PHYS_ADDR OFFSET(12) NUMBITS(40) [],

        /// NR - 禁止读位 (bit 61)
        NO_READ OFFSET(61) NUMBITS(1) [],

        /// NX - 禁止执行位 (bit 62)
        NO_EXEC OFFSET(62) NUMBITS(1) [],

        /// RPLV (bit 63)
        RPLV OFFSET(63) NUMBITS(1) [],
    ],
    /// LoongArch64 单页页表项 (Page Table Entry)
    ///
    /// 布局参考 LoongArch64 参考手册 5.4.2 节
    PTE [
        /// V - 有效位 (bit 0)
        VALID OFFSET(0) NUMBITS(1) [],

        /// D - 脏位 (bit 1)
        DIRTY OFFSET(1) NUMBITS(1) [],

        /// PLV - 特权级 (bits 2-3)
        PLV OFFSET(2) NUMBITS(2) [
            PLV0 = 0b00,  // 内核态
            PLV1 = 0b01,  // 特权级1
            PLV2 = 0b10,  // 特权级2
            PLV3 = 0b11   // 用户态
        ],

        /// 缓存属性 (bits 4-5)
        CACHE OFFSET(4) NUMBITS(2) [
            SUC = 0b00,  // 强序非缓存 (Strongly-ordered UnCached)
            CC  = 0b01,  // 一致性缓存 (Coherent Cached)
            WUC = 0b10   // 弱序非缓存 (Weakly-ordered UnCached)
        ],

        /// H/G - 共享位（bit 6）
        /// 在目录项中：H=1 表示大页映射
        /// 在页表项中：G=1 表示全局映射（此时 H 必须为 0）
        /// 注意：根据上下文区分是 H 位还是 G 位，不能同时为 1
        G OFFSET(6) NUMBITS(1) [],

        /// P - 存在位 (bit 7)
        PRESENT OFFSET(7) NUMBITS(1) [],

        /// W - 写位 (bit 8)
        WRITE OFFSET(8) NUMBITS(1) [],

        /// 物理页帧号 (bits 12-51)
        /// 注意: 根据 PDF, PPN 占据 bits [51:12]
        PHYS_ADDR OFFSET(12) NUMBITS(40) [],

        /// NR - 禁止读位 (bit 61)
        NO_READ OFFSET(61) NUMBITS(1) [],

        /// NX - 禁止执行位 (bit 62)
        NO_EXEC OFFSET(62) NUMBITS(1) [],

        /// RPLV (bit 63)
        RPLV OFFSET(63) NUMBITS(1) [],
    ],
];

/// 页表项寄存器类型别名
type PteRegister = ReadWrite<u64, PTE::Register>;

/// LoongArch64 页表项
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Entry(u64);

impl Entry {
    #[inline(always)]
    fn as_base(&self) -> &PteRegister {
        unsafe { &*(self as *const Self as *const PteRegister) }
    }

    #[inline(always)]
    fn as_dir(&self) -> &ReadWrite<u64, PTE_DIR::Register> {
        unsafe { &*(self as *const Self as *const _) }
    }

    /// 创建空页表项
    pub const fn empty() -> Self {
        Self(0)
    }

    pub(crate) fn debug(
        &self,
        is_dir: bool,
        f: &mut core::fmt::Formatter<'_>,
    ) -> core::fmt::Result {
        if is_dir {
            self.as_dir().debug().fmt(f)
        } else {
            self.as_base().debug().fmt(f)
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EntryDebug(Entry, bool);

impl Debug for EntryDebug {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.debug(self.1, f)
    }
}

impl PageTableEntry for Entry {
    fn from_config(config: page_table_generic::PteConfig) -> Self {
        let entry = Self::empty();

        // 目录项和页表项需要不同的处理
        if config.is_dir {
            if config.huge {
                entry.as_dir().modify(PTE_DIR::H::SET);

                // 页表项：设置完整的标志位和地址
                // 设置有效位和存在位
                if config.valid {
                    entry
                        .as_dir()
                        .modify(PTE_DIR::VALID::SET + PTE_DIR::PRESENT::SET);
                }

                if !config.read {
                    entry.as_dir().modify(PTE_DIR::NO_READ::SET);
                }

                // 设置可写标志和脏位
                if config.writable {
                    entry.as_dir().modify(PTE_DIR::WRITE::SET);
                }

                // 设置可执行标志
                if !config.executable {
                    entry.as_dir().modify(PTE_DIR::NO_EXEC::SET);
                }

                // 设置用户访问标志（PLV3 表示用户态）
                if config.lower {
                    entry.as_dir().modify(PTE_DIR::PLV::PLV3);
                } else {
                    entry.as_dir().modify(PTE_DIR::PLV::PLV0);
                }

                // 设置脏位
                if config.dirty {
                    entry.as_dir().modify(PTE_DIR::DIRTY::SET);
                } else {
                    entry.as_dir().modify(PTE_DIR::DIRTY::CLEAR);
                }

                // 设置物理地址
                let ppn = (config.paddr.raw() as u64) >> 12;
                entry.as_dir().modify(PTE_DIR::PHYS_ADDR.val(ppn));

                if config.global {
                    entry.as_dir().modify(PTE_DIR::G::SET);
                }

                // 设置内存属性
                let cache = match config.mem_attr {
                    MemAttributes::Device => PTE_DIR::CACHE::SUC, // SUC
                    MemAttributes::Normal | MemAttributes::PerCpu => PTE_DIR::CACHE::CC, // CC
                    MemAttributes::Uncached => PTE_DIR::CACHE::WUC, // WUC
                };
                entry.as_dir().modify(cache);
            } else {
                // 目录项：只设置地址部分，不设置标志位
                // 目录项的 bit [11:0] 应该全是 0，以便硬件正确计算地址
                let paddr = config.paddr.raw();
                entry
                    .as_dir()
                    .write(PTE_DIR::PHYS_ADDR.val((paddr >> 12) as u64));
            }
        } else {
            // 页表项：设置完整的标志位和地址
            // 设置有效位和存在位
            if config.valid {
                entry.as_base().modify(PTE::VALID::SET + PTE::PRESENT::SET);
            } else {
                entry
                    .as_base()
                    .modify(PTE::VALID::CLEAR + PTE::PRESENT::CLEAR);
            }

            if config.read {
                entry.as_base().modify(PTE::NO_READ::CLEAR);
            } else {
                entry.as_base().modify(PTE::NO_READ::SET);
            }

            // 设置可写标志和脏位
            if config.writable {
                entry.as_base().modify(PTE::WRITE::SET);
            } else {
                entry.as_base().modify(PTE::WRITE::CLEAR);
            }

            // 设置可执行标志
            if config.executable {
                entry.as_base().modify(PTE::NO_EXEC::CLEAR);
            } else {
                entry.as_base().modify(PTE::NO_EXEC::SET);
            }

            // 设置用户访问标志（PLV3 表示用户态）
            if config.lower {
                entry.as_base().modify(PTE::PLV::PLV3);
            } else {
                entry.as_base().modify(PTE::PLV::PLV0);
            }

            // 设置脏位
            if config.dirty {
                entry.as_base().modify(PTE::DIRTY::SET);
            } else {
                entry.as_base().modify(PTE::DIRTY::CLEAR);
            }

            // 设置物理地址
            let ppn = (config.paddr.raw() as u64) >> 12;
            entry.as_base().modify(PTE::PHYS_ADDR.val(ppn));

            // 设置全局标志（页表项使用 G 位，bit 6）
            if config.global {
                entry.as_base().modify(PTE::G::SET);
            }

            // 设置内存属性
            let cache = match config.mem_attr {
                MemAttributes::Device => 0b00,                         // SUC
                MemAttributes::Normal | MemAttributes::PerCpu => 0b01, // CC
                MemAttributes::Uncached => 0b10,                       // WUC
            };
            entry.as_base().modify(PTE::CACHE.val(cache));
        }

        entry
    }

    fn to_config(&self, is_dir: bool) -> page_table_generic::PteConfig {
        let valid = self.as_base().is_set(PTE::VALID);

        // 获取物理地址（关键：根据 is_dir 选择不同的布局）
        let paddr = if is_dir {
            // 目录项：使用 PTE_DIR 格式，bits [51:13]
            let raw_val = self.as_dir().read(PTE_DIR::PHYS_ADDR);
            (raw_val << 12) as usize
        } else {
            // 页表项：使用 PTE 格式，bits [51:12]
            let raw_val = self.as_base().read(PTE::PHYS_ADDR);
            (raw_val << 12) as usize
        };

        // 检查是否为大页（仅目录项）
        let huge = if is_dir {
            self.as_dir().is_set(PTE_DIR::H)
        } else {
            false
        };

        // 检查全局标志（仅页表项有 G 位，目录项没有）
        let global = if is_dir {
            false // 目录项没有全局标志
        } else {
            self.as_base().is_set(PTE::G)
        };

        // 内存属性
        let mem_attr = match self.as_base().read_as_enum(PTE::CACHE) {
            Some(PTE::CACHE::Value::SUC) => MemAttributes::Device,
            Some(PTE::CACHE::Value::CC) => MemAttributes::Normal,
            Some(PTE::CACHE::Value::WUC) => MemAttributes::Uncached,
            _ => MemAttributes::Normal,
        };

        page_table_generic::PteConfig {
            paddr: paddr.into(),
            valid,
            read: valid, // LoongArch64: 假设有效项可读
            writable: self.as_base().is_set(PTE::WRITE),
            executable: !self.as_base().is_set(PTE::NO_EXEC),
            lower: matches!(
                self.as_base().read_as_enum(PTE::PLV),
                Some(PTE::PLV::Value::PLV3)
            ),
            dirty: self.as_base().is_set(PTE::DIRTY),
            global,
            is_dir,
            huge,
            mem_attr,
        }
    }

    fn valid(&self) -> bool {
        self.as_base().is_set(PTE::VALID)
    }
}

impl core::fmt::Debug for Entry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let d = self.as_base().debug();
        d.fmt(f)
    }
}
