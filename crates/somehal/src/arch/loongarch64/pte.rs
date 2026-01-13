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

use super::addrspace::PAGE_OFFSET;

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
            // 目录项：只设置地址部分，不设置标志位
            // 目录项的 bit [11:0] 应该全是 0，以便硬件正确计算地址
            let paddr = config.paddr.raw() as usize;
            entry
                .as_dir()
                .modify(PTE_DIR::PHYS_ADDR.val((paddr >> 12) as u64));
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
                entry.as_base().modify(PTE::WRITE::SET + PTE::DIRTY::SET);
            } else {
                entry
                    .as_base()
                    .modify(PTE::WRITE::CLEAR + PTE::DIRTY::CLEAR);
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

/// 页表遍历结果
#[derive(Debug, Clone, Copy)]
pub struct WalkResult {
    /// 虚拟地址
    pub vaddr: usize,
    /// 最终物理地址
    pub paddr: usize,
    /// 是否是大页映射
    pub is_huge: bool,
    /// 大页级别 (0=PTE 4KB, 1=PMD 2MB, 2=PUD, 3=PGD 1GB)
    pub huge_level: usize,
}

/// 软件页表遍历 - 完全按照 QEMU 源码逻辑实现
/// 参考: QEMU 10.1.0 target/loongarch/tcg/tlb_helper.c
pub fn find_stlb(vaddr: usize) -> WalkResult {
    use super::addrspace::PAGE_OFFSET;
    use super::paging::{read_csr_pgdh, read_csr_pgdl, read_csr_pwctl0, read_csr_pwctl1};

    const VALEN: usize = 48;
    const PAGE_SHIFT: usize = 12;
    const PAGE_MASK: usize = (1 << PAGE_SHIFT) - 1;
    const TARGET_PHYS_MASK: usize = 0x0000ffffffffffff; // QEMU 使用的物理地址掩码

    println!("\n========== 硬件页表遍历模拟（QEMU 兼容）==========");
    println!("虚拟地址: {:#018x}", vaddr);

    // 读取 CSR 寄存器配置
    let pwctl0 = read_csr_pwctl0();
    let pwctl1 = read_csr_pwctl1();
    let asid = asid::read();

    // QEMU 使用 PWCL 和 PWCH 寄存器
    // PWCL (0x1c): PTBase[4:0], PTWidth[9:5], Dir0Base[14:10], Dir0Width[19:15], Dir1Base[24:20], Dir1Width[29:25]
    // PWCH (0x1d): Dir2Base[5:0], Dir2Width[11:6], Dir3Base[17:12], Dir3Width[23:18]

    let pt_base = (pwctl0 & 0x1f) as usize;
    let pt_width = ((pwctl0 >> 5) & 0x1f) as usize;
    let dir0_base = ((pwctl0 >> 10) & 0x1f) as usize;
    let dir0_width = ((pwctl0 >> 15) & 0x1f) as usize;
    let dir1_base = ((pwctl0 >> 20) & 0x1f) as usize;
    let dir1_width = ((pwctl0 >> 25) & 0x1f) as usize;

    let dir2_base = (pwctl1 & 0x3f) as usize;
    let dir2_width = ((pwctl1 >> 6) & 0x3f) as usize;
    let dir3_base = ((pwctl1 >> 12) & 0x3f) as usize;
    let dir3_width = ((pwctl1 >> 18) & 0x3f) as usize;

    println!("ASID: {:#x}", asid.asid());
    println!("PWCTL0: {:#018x}", pwctl0);
    println!("PWCTL1: {:#018x}", pwctl1);
    println!("页表配置:");
    println!("  PT(PTE):  base={}, width={}", pt_base, pt_width);
    println!("  Dir0(PMD): base={}, width={}", dir0_base, dir0_width);
    println!("  Dir1(PUD): base={}, width={}", dir1_base, dir1_width);
    println!("  Dir2(PGD): base={}, width={}", dir2_base, dir2_width);
    println!("  Dir3:     base={}, width={}", dir3_base, dir3_width);

    // 根据 VA[VALEN-1] 选择 PGDL 或 PGDH
    let use_high_half = (vaddr >> (VALEN - 1)) & 1 == 1;
    let mut table_paddr = if use_high_half {
        println!("使用高地址空间页表 (PGDH)");
        read_csr_pgdh() as usize
    } else {
        println!("使用低地址空间页表 (PGDL)");
        read_csr_pgdl() as usize
    };

    println!("页表基址: {:#018x}", table_paddr);

    // QEMU 的目录项遍历逻辑
    // 从最高级开始，遍历到 PTE 级别
    // 注意：QEMU 的 lddir 指令参数：0=PWCTL0.Dir0, 1=PWCTL0.Dir1, 2=PWCTL1.Dir2, 3=PWCTL1.Dir3

    // 定义各级遍历配置，按照 QEMU 的 get_directory_entry 逻辑
    // 硬件 TLB refill 使用：lddir 2 (PGD), lddir 1 (PUD), lddir 0 (PMD), ldpte (PTE)
    // 使用静态数组避免在 no_std 环境中使用 vec
    let walk_levels: [(usize, &str, usize, usize); 3] = [
        (2, "PGD", dir2_base, dir2_width), // lddir 2
        (1, "PUD", dir1_base, dir1_width), // lddir 1
        (0, "PMD", dir0_base, dir0_width), // lddir 0
    ];

    for (hw_level, level_name, base, width) in walk_levels.iter() {
        let base = *base;
        let width = *width;
        println!(
            "\n--- {} 级别 (hw_level={}, base={}, width={}) ---",
            level_name, hw_level, base, width
        );

        // QEMU 的索引计算：index = (vaddr >> base) & ((1 << width) - 1)
        let index = (vaddr >> base) & ((1usize << width) - 1);
        println!(
            "  索引计算: vaddr[{}:{}] = {:#x}",
            base + width - 1,
            base,
            index
        );

        // QEMU 的物理地址计算：phys = base | (index << 3)
        // 注意：这里使用 table_paddr 作为基地址
        let entry_phys_addr = table_paddr | (index << 3);
        println!(
            "  目录项物理地址: base({:#x}) | (index({:#x}) << 3) = {:#x}",
            table_paddr, index, entry_phys_addr
        );

        // 转换为虚拟地址读取
        let entry_vaddr = entry_phys_addr + PAGE_OFFSET;
        let entry_ptr = entry_vaddr as *const u64;

        // 读取目录项值，并应用 TARGET_PHYS_MASK
        unsafe { core::arch::asm!("dbar 0", options(nostack, nomem)) };
        let entry_val_raw = unsafe { core::ptr::read_volatile(entry_ptr) };
        let entry_val = entry_val_raw & TARGET_PHYS_MASK as u64;

        println!("  目录项虚拟地址: {:#018x}", entry_vaddr);
        println!("  原始读取值: {:#018x}", entry_val_raw);
        println!("  应用物理掩码后: {:#018x}", entry_val);

        // QEMU 的页表项检查（参考 loongarch_map_tlb_entry）
        let tlb_v = (entry_val & 0x1) != 0; // bit 0: V (Valid)
        let tlb_d = (entry_val & 0x2) != 0; // bit 1: D (Dirty)
        let tlb_plv = (entry_val >> 2) & 0x3; // bit [2:3]: PLV
        let tlb_nr = (entry_val >> 61) & 0x1; // bit 61: NR (No Read)
        let tlb_nx = (entry_val >> 62) & 0x1; // bit 62: NX (No Execute)
        let tlb_rplv = (entry_val >> 63) & 0x1; // bit 63: RPLV

        println!("  页表项标志检查:");
        println!(
            "    V (Valid):      {} ({})",
            tlb_v,
            if tlb_v { "✓" } else { "✗ 无效" }
        );
        println!("    D (Dirty):      {}", tlb_d);
        println!("    PLV (Priv):     {}", tlb_plv);
        println!("    NR (No Read):   {}", tlb_nr);
        println!("    NX (No Exec):   {}", tlb_nx);
        println!("    RPLV:          {}", tlb_rplv);

        // QEMU 检查 V 位
        if !tlb_v {
            panic!("❌ QEMU 检查失败：页表项 V=0 (无效) - 这会导致 ret 3");
        }

        // 检查 NX 位（No Execute）
        if tlb_nx != 0 {
            panic!("❌ QEMU 检查失败：页表项 NX=1 (不可执行) - 这会导致 ret 6");
        }

        // 检查 NR 位（No Read）
        if tlb_nr != 0 {
            panic!("❌ QEMU 检查失败：页表项 NR=1 (不可读) - 这会导致 ret 5");
        }

        // 检查 bit 6 (HUGE 位，参考 helper_ldpte)
        let bit6_set = (entry_val & (1 << 6)) != 0;
        println!(
            "    Bit 6 (HUGE):   {}",
            if bit6_set {
                "1 (大页)"
            } else {
                "0 (普通页)"
            }
        );

        // 提取物理页帧号（PPN）
        // QEMU: PPN 在 bits [51:12]
        let ppn = (entry_val >> 12) & ((1u64 << 40) - 1);
        println!("    PPN (bits[51:12]): {:#x}", ppn);

        // 计算下一级页表的物理地址
        let next_table_paddr = ((ppn << 12) as usize) & TARGET_PHYS_MASK;
        println!(
            "  -> 下一级页表物理地址: PPN({:#x}) << 12 = {:#x}",
            ppn, next_table_paddr
        );

        // 检查是否是大页（bit 6 = 1）
        if bit6_set {
            // 大页处理（QEMU 的 helper_ldpte 逻辑）
            // 大页需要特殊处理：获取页大小，清除 HUGE 位，移动 HGLOBAL 到 G
            let ps = base + width - 1; // 页大小 = base + width - 1
            println!("  -> 检测到大页，页大小: 2^{} bytes", ps);

            // 大页直接返回
            let page_size = 1usize << ps;
            let offset_in_page = vaddr & (page_size - 1);
            let final_paddr = (next_table_paddr as usize) + offset_in_page;

            println!("✓ 大页映射成功");
            println!("  物理地址: {:#018x}", final_paddr);
            return WalkResult {
                vaddr,
                paddr: final_paddr,
                is_huge: true,
                huge_level: *hw_level,
            };
        }

        // 更新 table_paddr 为下一级页表
        table_paddr = next_table_paddr;
    }

    // 最后一级：PTE (使用 ldpte 指令)
    println!("\n--- PTE 级别 (ldpte) ---");
    let pt_base = pt_base;
    let pt_width = pt_width;

    // QEMU 的 ldpte 指令逻辑
    let badv = vaddr;
    let mut ptindex = (badv >> pt_base) & ((1usize << pt_width) - 1);
    ptindex = ptindex & !0x1; // clear bit 0

    let ptoffset0 = ptindex << 3;
    let ptoffset1 = (ptindex + 1) << 3;

    println!(
        "  PTE 索引: vaddr[{}:{}] = {:#x} (清除bit0后)",
        pt_base + pt_width - 1,
        pt_base,
        ptindex
    );
    println!("  PTE offset0: {:#x}", ptoffset0);
    println!("  PTE offset1: {:#x}", ptoffset1);

    // 读取 PTE（QEMU 读取一对页表项）
    let pte_phys_addr0 = table_paddr | ptoffset0;

    // 🔍 调试：模拟 lddir/ldpte 硬件遍历的地址计算
    // PMD 目录项值（这是 lddir 1 返回的值）
    let pmd_entry_val = 0x0000000005413191u64; // 从测试输出中获取
    println!("  🔍 硬件遍历模拟（lddir/ldpte）:");
    println!("    PMD 目录项值 (lddir 1 返回): {:#018x}", pmd_entry_val);
    println!("    低12位标志位: {:#03x}", pmd_entry_val & 0xFFF);

    // QEMU 的 ldpte 计算：phys = base | offset
    // 其中 base 是 lddir 返回的目录项值（包含低12位标志位）
    let hw_pte_phys0_wrong = (pmd_entry_val as usize) | ptoffset0;
    println!(
        "    硬件计算（错误）: base({:#x}) | offset({:#x}) = {:#x}",
        pmd_entry_val, ptoffset0, hw_pte_phys0_wrong
    );

    // 正确的计算应该是：清除低12位后再 OR
    let hw_pte_phys0_correct = ((pmd_entry_val & !0xFFF) as usize) | ptoffset0;
    println!(
        "    正确计算: (base & ~0xFFF) | offset = {:#x}",
        hw_pte_phys0_correct
    );
    println!("    软件遍历: {:#x}", pte_phys_addr0);
    println!("    差异: {:#x}", hw_pte_phys0_wrong ^ pte_phys_addr0);

    let pte_vaddr0 = pte_phys_addr0 + PAGE_OFFSET;
    let pte_ptr0 = pte_vaddr0 as *const u64;

    unsafe { core::arch::asm!("dbar 0", options(nostack, nomem)) };
    let pte_val_raw0 = unsafe { core::ptr::read_volatile(pte_ptr0) };
    let pte_val0 = pte_val_raw0 & TARGET_PHYS_MASK as u64;

    println!("  PTE[0] 物理地址: {:#x}", pte_phys_addr0);
    println!("  PTE[0] 原始值: {:#018x}", pte_val_raw0);
    println!("  PTE[0] 掩码后: {:#018x}", pte_val0);

    // QEMU 的 PTE 检查
    let tlb_v = (pte_val0 & 0x1) != 0;
    let tlb_d = (pte_val0 & 0x2) != 0;
    let tlb_plv = (pte_val0 >> 2) & 0x3;
    let tlb_nr = (pte_val0 >> 61) & 0x1;
    let tlb_nx = (pte_val0 >> 62) & 0x1;
    let tlb_rplv = (pte_val0 >> 63) & 0x1;

    println!("  PTE[0] 标志:");
    println!("    V (Valid):    {}", tlb_v);
    println!("    D (Dirty):    {}", tlb_d);
    println!("    PLV:         {}", tlb_plv);
    println!("    NR (No Read): {}", tlb_nr);
    println!("    NX (No Exec): {}", tlb_nx);

    if !tlb_v {
        panic!("❌ PTE[0] V=0 - 页表项无效，这会导致 QEMU ret 3");
    }

    if tlb_nx != 0 {
        panic!("❌ PTE[0] NX=1 - 不可执行，这会导致 QEMU ret 6");
    }

    // 提取物理地址并计算最终物理地址
    let ppn = (pte_val0 >> 12) & ((1u64 << 40) - 1);
    let phys_page_base = ((ppn << 12) as usize) & TARGET_PHYS_MASK;
    let offset_in_page = vaddr & PAGE_MASK;
    let final_paddr = phys_page_base + offset_in_page;

    println!("  PPN: {:#x}", ppn);
    println!("  物理页基址: {:#x}", phys_page_base);
    println!("  页内偏移: {:#x}", offset_in_page);
    println!("  最终物理地址: {:#x}", final_paddr);
    println!("==========================================\n");

    WalkResult {
        vaddr,
        paddr: final_paddr,
        is_huge: false,
        huge_level: 0,
    }
}
