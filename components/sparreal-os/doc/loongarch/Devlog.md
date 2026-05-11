# 开发记录

## 启动过程

LoongArch 架构的启动过程与其他主流架构类似，但也有其独特之处。以下是 LoongArch 架构的启动过程的详细描述：

### 整体流程概览

```text
┌─────────────────────────────────────────────────────────────┐
│ 1. UEFI 固件启动 (QEMU/硬件)                                │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. 加载 EFI 应用程序 (BOOTLOONGARCH64.EFI)                  │
│    - 识别 PE/COFF 格式                                      │
│    - 解析入口点: efi_pe_entry                               │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. EFI 入口点执行 ([efi_stub/mod.rs:19-47](../../crates/someboot/src/efi_stub/mod.rs#L19-L47)) │
│    - 重定位: relocate()                                     │
│    - 设置 EFI 环境句柄                                      │
│    - 查找 ACPI RSDP                                         │
│    - 获取命令行参数                                         │
│    - 退出 Boot Services                                     │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. 跳转到内核入口 kernel_entry([entry.rs:9-85](../../crates/someboot/src/arch/loongarch64/entry.rs#L9-L85)) │
│    参数: a0=1(efi_boot), a1=cmdline, a2=systemtable       │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ 5. 低级初始化 (汇编语言)                                     │
│    5.1 设置直接映射窗口 (DMW)                               │
│        - CSR_DMW0: 0x8000_0000_0000 (Uncacheable)          │
│        - CSR_DMW1: 0x9000_0000_0000 (Cacheable, 正常内存)  │
│        - CSR_DMW2: 0xa000_0000_0000 (可写内存)             │
│    5.2 跳转到虚拟地址 (JUMP_TO_VIRT_ADDR)                   │
│        - 计算当前 PC 的虚拟地址                             │
│        - 使用 CACHE_BASE (0x9000_0000_0000) 作为基地址     │
│    5.3 启用分页 (CRMD.PG = 1)                               │
│        - 设置特权级 PLV0                                    │
│        - 启用中断 (PRMD.PIE = 1)                            │
│    5.4 清空 .bss 段                                         │
│    5.5 保存固件参数                                         │
│    5.6 设置栈指针 (__cpu0_stack_top)                        │
│    5.7 刷新指令和数据缓存 (ibar, dbar)                      │
└────────────────────────┬────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ 6. 执行 Rust 主函数 ([entry.rs:87-97](../../crates/someboot/src/arch/loongarch64/entry.rs#L87-L97)) │
│    - 再次重定位: relocate()                                 │
│    - 打印启动信息                                           │
│    - 调用 prime_entry() 进入平台初始化                      │
└─────────────────────────────────────────────────────────────┘
```

### 关键技术细节

#### 1. PE/COFF 头部结构 ([head.rs:11-121](../../crates/someboot/src/arch/loongarch64/head.rs#L11-L121))

内核镜像以 PE/COFF 格式开头，遵循 Linux 内核的 EFISTUB 协议:

```rust
// 偏移 0x00: MS-DOS 头部 (0x5A4D = "MZ")
// 偏移 0x08: 内核入口点物理地址 (_kernel_entry)
// 偏移 0x10: 内核镜像有效大小 (_kernel_asize)
// 偏移 0x18: 物理加载地址偏移 (PHYS_LINK_KADDR)
// 偏移 0x38: Linux PE 魔数 (0x818223cd)
// 偏移 0x3C: PE 头部偏移
```

**PE 头部关键字段**:

- **Machine**: `IMAGE_FILE_MACHINE_LOONGARCH64` (0x6264)
- **EntryPoint**: `efi_pe_entry` 函数
- **Subsystem**: `IMAGE_SUBSYSTEM_EFI_APPLICATION` (10)
- **Sections**: `.text` (代码) 和 `.data` (数据)

#### 2. 地址空间布局 ([addrspace.rs](../../crates/someboot/src/arch/loongarch64/addrspace.rs))

LoongArch64 使用 **直接映射窗口 (Direct Mapped Windows, DMW)** 实现物理内存的直接映射:

| DMW 寄存器 | 虚拟地址段 | 物理属性 | 用途 | 基地址 |
| :--- | :--- | :--- | :--- | :--- |
| CSR_DMW0 | 0x8000 | Uncacheable | MMIO/设备访问 | 0x8000_0000_0000 |
| CSR_DMW1 | 0x9000 | Cacheable | 正常内存访问 | 0x9000_0000_0000 |
| CSR_DMW2 | 0xa000 | Writable | 可写内存 | 0xa000_0000_0000 |

**关键宏定义**:

- `PABITS = 48`: 物理地址位数 (支持 256TB 内存)
- `CACHE_BASE = 0x9000_0000_0000`: 可缓存内存基址
- `UNCACHE_BASE = 0x8000_0000_0000`: 不可缓存访问基址

#### 3. 重定位机制 ([relocate.rs](../../crates/someboot/src/arch/loongarch64/relocate.rs))

由于 EFI 可能将内核加载到任意物理地址，需要两次重定位:

**第一次重定位** (在 `efi_pe_entry` 中):

- 计算加载偏移: `实际地址 - VM_LOAD_ADDRESS`
- 应用 `.rela.dyn` 段的重定位项
- 使用 `R_LARCH_RELATIVE` 类型重定位

**第二次重定位** (在 `rust_main` 中):

- 跳转到虚拟地址后再次执行
- 确保所有符号地址正确

#### 4. 链接脚本布局 ([link.ld](../../crates/someboot/src/arch/loongarch64/link.ld))

内存布局 (虚拟地址空间):

```text
0xb000_0000_0000 +-------------------+ ← VM_LOAD_ADDRESS
                 | .head.text (PE头) |
                 +-------------------+
                 | .text (代码段)    |
                 +-------------------+
                 | .exception.vectors| ← 必须在 64KB 边界对齐
                 | (0x10000 大小)    |
                 +-------------------+
                 | .rodata (只读)    |
                 +-------------------+
                 | .data (数据)      |
                 +-------------------+
                 | .bss (零初始化)   |
                 +-------------------+
                 | CPU0 栈 (16KB)    |
                 +-------------------+
                 +-------------------+ ← __kernel_code_end
```

**特殊段**:

- `.head.text`: PE/COFF 头部，必须在镜像开头
- `.exception.vectors`: 异常向量表，64KB 对齐
- `.la_abs`: LoongArch 绝对寻址重定位
- `.rela.dyn` / `.relr.dyn`: 动态重定位信息

#### 5. EFI 服务生命周期 ([efi_stub/mod.rs](../../crates/someboot/src/efi_stub/mod.rs))

```text
efi_pe_entry
    ↓
[UEFI 服务可用]
    ├─ 设置 image_handle 和 system_table
    ├─ 查找 ACPI RSDP (ConfigTable)
    ├─ 获取 LoadOptions (命令行参数)
    └─ 退出 Boot Services (获取内存映射)
        ↓
[UEFI 服务不可用]
    ├─ 设置内存映射
    └─ 跳转到 kernel_entry
```

**关键操作**:

1. **查找 ACPI RSDP**: 遍历 UEFI 配置表，优先使用 ACPI 2.0 RSDP
2. **退出 Boot Services**: `boot::exit_boot_services()` 获取内存控制权
3. **内存映射**: 将 EFI 内存类型转换为内核内存管理器格式

### 与标准 Linux 内核的对比

| 特性 | Linux 内核 | Sparreal OS |
| :--- | :--- | :--- |
| **入口函数** | `kernel_entry` (汇编) | `efi_pe_entry` (Rust) → `kernel_entry` |
| **PE 头部** | 完整 EFISTUB | 精简版 PE 头部 |
| **重定位** | 汇编实现 | Rust + 汇编宏实现 |
| **语言** | C + 汇编 | Rust + 内联汇编 |
| **地址布局** | 0x9000_0000_0000 (Linux 标准) | 0xb000_0000_0000 (自定义) |
| **异常向量** | 0x9000_0000_0000 | 在 .text 段后，64KB 对齐 |

### 参考资源

**官方文档**:

- [LoongArch Linux 启动协议](https://www.kernel.org/doc/html/latest/loongarch/booting.html)
- [龙芯架构参考手册](https://loongson.github.io/LoongArch-Documentation/)

**社区资源**:

- [3A5000 UEFI 开发](https://loongarch.dev/zh-cn/posts/3a5000-uefi/)
- [LoongArch UEFI 固件构建指南](https://hev.cc/posts/2024/build-uefi-firmware-for-qemu-loongarch/)

**相关代码**:

- Linux 内核: `arch/loongarch/kernel/head.S`
- GRUB: `grub-core/loader/loongarch64/efi/linux.c`

---

## EFI Stub 深度解析

### 什么是 EFI Stub？

**EFI Stub** 是一种将内核镜像构建为 EFI 应用程序的技术，使内核能够被 UEFI 固件直接加载和执行，无需传统引导加载程序（如 GRUB）。

**核心优势**:

1. **简化启动流程**: 跳过引导加载程序，直接由固件加载
2. **统一接口**: 跨平台使用标准 EFI ABI
3. **硬件信息获取**: 直接访问 EFI 提供的内存映射、ACPI 表等
4. **灵活性**: 仍可通过引导加载程序实现多系统引导

### Sparreal OS 的 EFI Stub 实现架构

```text
┌──────────────────────────────────────────────────────────────┐
│                    PE/COFF 头部 (_head)                      │
│  - MS-DOS 头部 (0x5A4D "MZ")                                  │
│  - 内核入口点 (_kernel_entry)                                 │
│  - 镜像大小 (_kernel_asize)                                   │
│  - Linux PE 魔数 (0x818223cd)                                 │
│  - 完整 PE 头部 (Machine, EntryPoint, Sections)              │
└────────────────────────┬─────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────────────┐
│              EFI 入口点 (efi_pe_entry)                        │
│  [crates/someboot/src/efi_stub/mod.rs:19-47](../../crates/someboot/src/efi_stub/mod.rs#L19-L47) │
│                                                              │
│  1. relocate()           - 首次重定位 (物理地址)              │
│  2. set_image_handle()   - 保存 EFI 镜像句柄                 │
│  3. set_system_table()   - 保存 EFI 系统表                   │
│  4. find_acpi_rsdp()     - 查找 ACPI RSDP 地址               │
│  5. get_load_options()   - 获取命令行参数                     │
│  6. exit_boot_services() - 退出 Boot Services                │
│  7. setup_memory_map()   - 设置内存映射                       │
│  8. kernel_entry()       - 跳转到内核入口                     │
└────────────────────────┬─────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────────────┐
│           汇编级初始化 (kernel_entry)                         │
│  [crates/someboot/src/arch/loongarch64/entry.rs:9-85](../../crates/someboot/src/arch/loongarch64/entry.rs#L9-L85) │
│                                                              │
│  1. CSR_DMW[0-2]         - 设置直接映射窗口                   │
│  2. JUMP_TO_VIRT_ADDR    - 跳转到虚拟地址                     │
│  3. CRMD.PG = 1          - 启用分页                           │
│  4. 清空 .bss            - 零初始化 BSS 段                    │
│  5. 保存固件参数         - a0, a1, a2 寄存器                 │
│  6. 设置栈指针           - __cpu0_stack_top                  │
│  7. ibar/dbar            - 刷新缓存                           │
└────────────────────────┬─────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────────────┐
│             Rust 主函数 (rust_main)                          │
│  [crates/someboot/src/arch/loongarch64/entry.rs:87-97](../../crates/someboot/src/arch/loongarch64/entry.rs#L87-L97) │
│                                                              │
│  1. relocate()           - 二次重定位 (虚拟地址)              │
│  2. 打印启动信息         - "Rust main."                      │
│  3. prime_entry()        - 进入平台初始化                    │
└──────────────────────────────────────────────────────────────┘
```

### 重定位机制详解

#### 为什么需要两次重定位？

**场景分析**:

```text
┌─────────────────────────────────────────────────────────────┐
│ 链接时 (编译)                                                │
├─────────────────────────────────────────────────────────────┤
│ 代码链接地址: 0xb000_0000_0000 (虚拟地址)                   │
│ 数据链接地址: 0xb000_0000_0000 (虚拟地址)                   │
│ 全局变量 g:  链接地址 = 0xb000_0001_2340                    │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ EFI 加载时                                                  │
├─────────────────────────────────────────────────────────────┤
│ 实际加载地址: 0x8000_0000 (物理地址)                        │
│ 代码在: 0x8000_0000 + offset                               │
│ 全局变量 g: 实际地址 = 0x8000_0000 + offset + 0x12340       │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 第一次重定位 (efi_pe_entry)                                 │
├─────────────────────────────────────────────────────────────┤
│ 目的: 修正物理地址下的指针                                  │
│ 计算: load_offset = 实际地址 - 链接地址                    │
│       = 0x8000_0000 - 0xb000_0000_0000                     │
│       = -0xaffff_fffff_0000                                │
│ 操作: 遍历 .rela.dyn 段，对每个指针:                       │
│       *ptr = r_addend + load_offset                        │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 跳转到虚拟地址 (JUMP_TO_VIRT_ADDR)                          │
├─────────────────────────────────────────────────────────────┤
│ 设置 DMW 窗口: CSR_DMW1 = 0x9000... (直接映射)             │
│ 计算: virt_addr = CACHE_BASE | phys_addr                   │
│       = 0x9000_0000_0000 | 0x8000_0000                     │
│       = 0x9000_8000_0000                                   │
│ 执行: jirl $zero, $t0, 0xc (跳转)                          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 第二次重定位 (rust_main)                                    │
├─────────────────────────────────────────────────────────────┤
│ 目的: 修正虚拟地址下的指针                                  │
│ 计算: load_offset = 0 (已经在虚拟地址)                     │
│ 操作: 将所有指针设置为链接时地址                            │
│       *ptr = r_addend (原始加数)                           │
└─────────────────────────────────────────────────────────────┘
```

#### 重定位代码实现

**LoongArch64 实现** ([relocate.rs:38-61](../../crates/someboot/src/arch/loongarch64/relocate.rs#L38-L61)):

```rust
pub fn relocate_with_offset(offset: i64) {
    unsafe {
        crate::elf::apply_reloc(
            offset,
            sym_lma!(__rela_dyn_begin) as _,
            sym_lma!(__rela_dyn_end) as _,
            R_LARCH_RELATIVE,  // 3
        );
    }

    // 刷新指令与数据缓存
    unsafe {
        asm!("ibar 0", options(nostack));
        asm!("dbar 0", options(nostack));
    }
}
```

**通用 ELF 重定位函数** ([elf.rs:19-34](../../crates/someboot/src/elf.rs#L19-L34)):

```rust
pub unsafe fn apply_reloc(load_offset: i64, start: *mut u8, end: *const u8, r_type: u32) {
    if load_offset == 0 {
        return;  // 无需重定位
    }

    let num_entries = (end as usize - start as usize) / size_of::<Rela>();
    let relocations = core::slice::from_raw_parts_mut(start as *mut Rela, num_entries);

    for reloc in relocations {
        if reloc.r_type_raw() == r_type {
            // 计算重定位后的地址
            let addr = (reloc.r_offset as i64 + load_offset) as usize as *mut usize;
            let (val, _) = (reloc.r_addend as u64).overflowing_add(load_offset as u64);
            *addr = val as usize;
        }
    }
}
```

### 内存映射转换

#### EFI 内存类型到内核内存类型

内存类型映射表:

| EFI 内存类型 | 内核类型 | 说明 |
| :--- | :--- | :--- |
| `CONVENTIONAL` | `Free` | 常规可用内存 |
| `BOOT_SERVICES_CODE/DATA` | `Free` | 引导服务内存（可回收） |
| `LOADER_CODE/DATA` | `Free` | 加载器内存（可回收） |
| `MMIO/MMIO_PORT_SPACE` | `Mmio` | 内存映射 I/O |
| `RUNTIME_SERVICES_CODE/DATA` | `Reserved` | UEFI 运行时服务 |
| `ACPI_RECLAIM/NON_VOLATILE` | `Reserved` | ACPI 表 |
| `RESERVED/UNUSABLE` | `Reserved` | 保留/不可用内存 |

---

## 跨平台架构设计

### 架构抽象层设计

Sparreal OS 使用 Rust 的 trait 系统实现跨平台抽象 ([lib.rs:48-91](../../crates/someboot/src/lib.rs#L48-L91)):

```rust
pub trait ArchTrait {
    // 页表类型 (关联类型，由各架构实现)
    type PT<A: FrameAllocator>: PageTableOp<A>;

    // 内核代码段访问
    fn kernel_code() -> &'static [u8];

    // 虚拟地址转换
    fn _va(paddr: usize) -> *mut u8;  // 物理地址 → 虚拟地址
    fn _io(paddr: usize) -> *mut u8;  // I/O 物理地址 → 映射地址

    // 地址转换
    fn virt_to_phys(vaddr: *const u8) -> usize;
    fn ioremap(paddr: usize, size: usize) -> *mut u8;

    // MMU 控制
    fn is_mmu_enabled() -> bool;
    fn enable_paging();

    // 页表管理
    fn create_page_table<A: FrameAllocator>(allocator: A) -> Self::PT<A>;
    fn kernel_page_table() -> PageTableInfo;

    // 系统定时器
    fn systimer_enable();
    fn systimer_set_interval(ticks: usize);
    fn systimer_ack();

    // 中断控制
    fn irq_all_set_enable(enable: bool);

    // 电源管理
    fn shutdown() -> !;
}
```

### LoongArch64 vs AArch64 启动流程对比

#### 启动头部格式差异

| 特性 | LoongArch64 | AArch64 |
| :--- | :--- | :--- |
| **头部格式** | PE/COFF (EFI 应用) | ARM64 镜像格式 |
| **头部大小** | 512+ 字节 | 64 字节 |
| **魔数** | `0x5A4D` ("MZ") | `"ARM\x64"` |
| **入口点** | `efi_pe_entry` | 直接 `kernel_entry` |
| **对齐要求** | 512 字节 (PECOFF) | 2KB (页) |

**LoongArch64 头部** ([head.rs:14-37](../../crates/someboot/src/arch/loongarch64/head.rs#L14-L37)):

```rust
.word IMAGE_DOS_SIGNATURE        // "MZ"
.org 0x8
.dword _kernel_entry             // 入口点
.dword _kernel_asize              // 镜像大小
.dquad phys_link_kaddr           // 加载地址
.org 0x38
.long LINUX_PE_MAGIC             // 0x818223cd
.long 4f - _head                 // PE 偏移
```

**AArch64 头部** ([aarch64/head.rs:16-36](../../crates/someboot/src/arch/aarch64/head.rs#L16-L36)):

```rust
"nop",                            // code0
"bl {entry}",                     // code1 (跳转到入口)
.quad 0,                          // text_offset
.quad __kernel_load_end - _head,  // image_size
.quad {flags},                    // flags
.ascii "ARM\\x64",                 // magic
```

#### 入口函数签名差异

**LoongArch64** ([entry.rs:9-15](../../crates/someboot/src/arch/loongarch64/entry.rs#L9-L15)):

```rust
pub unsafe extern "C" fn kernel_entry(
    efi_boot: usize,      // a0: EFI 启动标志
    cmdline: *const u8,   // a1: 命令行指针
    systemtable: *const c_void,  // a2: EFI 系统表
) -> !
```

**AArch64** ([aarch64/entry.rs:9-9](../../crates/someboot/src/arch/aarch64/entry.rs#L9-L9)):

```rust
pub unsafe extern "C" fn kernel_entry(
    _fdt_addr: usize,     // x0: FDT (设备树) 地址
) -> !
```

**关键区别**:

- **LoongArch64**: 通过 EFI 获取硬件信息（ACPI、内存映射）
- **AArch64**: 通过 FDT (Flattened Device Tree) 获取硬件信息

#### 地址映射机制差异

**LoongArch64 - 直接映射窗口 (DMW)**:

```rust
// 物理地址直接映射到虚拟地址高段
pub const CSR_DMW0_BASE: usize = 0x8000_0000_0000;  // Uncacheable
pub const CSR_DMW1_BASE: usize = 0x9000_0000_0000;  // Cacheable
pub const CSR_DMW2_BASE: usize = 0xa000_0000_0000;  // Writable

// 无需页表即可访问:
//   phys_addr 0x1_0000 → virt_addr 0x9000_0001_0000
```

**AArch64 - 页表映射**:

```rust
// 必须通过页表进行地址转换
// 1. 创建页表层次结构 (4 级页表)
// 2. 设置 TTBR1_EL1 (内核页表基址)
// 3. 启用 MMU (SCTLR_EL1.M = 1)
// 4. 之后才能访问高地址虚拟内存
```

**对比表**:

| 特性 | LoongArch64 DMW | AArch64 页表 |
| :--- | :--- | :--- |
| **初始化复杂度** | 低 (设置 CSR 寄存器) | 高 (创建页表层次) |
| **地址计算** | 简单 (按位或) | 复杂 (多级查表) |
| **灵活性** | 低 (固定映射) | 高 (自定义映射) |
| **性能** | 高 (硬件直接映射) | 中 (需查表) |
| **适用场景** | 早期启动 | 完整内存管理 |

#### 重定位实现差异

**LoongArch64** ([relocate.rs:38-61](../../crates/someboot/src/arch/loongarch64/relocate.rs#L38-L61)):

```rust
pub fn relocate() {
    let offset = sym_lma!(_head) as i64 - VM_LOAD_ADDRESS as i64;
    relocate_with_offset(offset);
}

// 重定位类型: R_LARCH_RELATIVE = 3
```

**AArch64** ([aarch64/relocate.rs:15-25](../../crates/someboot/src/arch/aarch64/relocate.rs#L15-L25)):

```rust
pub fn apply() {
    unsafe {
        OFFSET = get_load_offset();
        crate::elf::apply_reloc(
            OFFSET,
            ext_sym_addr!(__rela_dyn_begin) as _,
            ext_sym_addr!(__rela_dyn_end) as _,
            R_AARCH64_RELATIVE,  // 1027
        );
    }
}

pub fn reset() {
    unsafe {
        crate::elf::reset(R_AARCH64_RELATIVE);
    }
}
```

**关键区别**:

- **重定位类型**: `R_LARCH_RELATIVE` (3) vs `R_AARCH64_RELATIVE` (1027)
- **缓存管理**: LoongArch64 需要 `ibar/dbar`，AArch64 使用 `ic iallu`
- **重定位时机**: LoongArch64 重定位两次，AArch64 在启用 MMU 后重置

### 通用代码复用策略

#### 1. ELF 重定位 ([elf.rs](../../crates/someboot/src/elf.rs))

完全跨平台，只需指定架构特定的重定位类型:

```rust
// 所有架构共享
pub unsafe fn apply_reloc(load_offset: i64, start: *mut u8, end: *const u8, r_type: u32) {
    // ... 通用重定位逻辑
}

// 各架构指定自己的类型
#[cfg(target_arch = "loongarch64")]
const R_ARCH_RELATIVE: u32 = R_LARCH_RELATIVE;  // 3

#[cfg(target_arch = "aarch64")]
const R_ARCH_RELATIVE: u32 = R_AARCH64_RELATIVE;  // 1027
```

#### 2. EFI Stub ([efi_stub/](../../crates/someboot/src/efi_stub/))

平台无关，仅在使用 `--features efi` 时编译:

```rust
// memmap.rs - 跨平台内存映射转换
pub fn setup_memory_map<'a>(
    mems: impl Iterator<Item = &'a MemoryDescriptor>,
) -> anyhow::Result<()> {
    add_memory_descriptors(mems.map(|memory| match memory.ty {
        MemoryType::CONVENTIONAL => MemoryDescriptor {
            name: "RAM",
            physical_start: memory.phys_start as _,
            size_in_bytes: memory.page_count as usize * page_size(),
            memory_type: crate::mem::MemoryType::Free,
        },
        // ... 其他类型
    }))
}
```

### 平台特定代码隔离

#### 目录结构

```text
crates/someboot/src/
├── lib.rs              # 通用 trait 和函数
├── elf.rs              # 通用 ELF 重定位
├── consts.rs           # 通用常量
├── cmdline.rs          # 命令行解析
├── mem/                # 通用内存管理
├── efi_stub/           # EFI Stub (平台无关)
└── arch/
    ├── loongarch64/    # LoongArch64 实现
    │   ├── mod.rs
    │   ├── head.rs     # PE 头部
    │   ├── entry.rs    # 入口函数
    │   ├── relocate.rs # 重定位
    │   ├── addrspace.rs# 地址空间
    │   └── paging.rs   # 页表
    │
    └── aarch64/        # AArch64 实现
        ├── mod.rs
        ├── head.rs     # ARM64 头部
        ├── entry.rs    # 入口函数
        ├── relocate.rs # 重定位
        ├── addrspace.rs# 地址空间
        └── paging/     # 页表管理
```

#### 条件编译示例 ([lib.rs:19-29](../../crates/someboot/src/lib.rs#L19-L29))

```rust
// 根据目标架构选择对应模块
#[cfg(target_arch = "loongarch64")]
#[path = "arch/loongarch64/mod.rs"]
pub mod arch;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64/mod.rs"]
pub mod arch;

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64/mod.rs"]
pub mod arch;
```

**优势**:

- 编译时选择架构，零运行时开销
- 每个架构独立目录，易于维护
- 共享通用代码（`elf.rs`, `efi_stub/`, `mem/` 等）

### 最佳实践总结

1. **抽象共性**: 使用 trait 定义统一接口
2. **隔离差异**: 每个架构独立目录
3. **共享代码**: ELF 重定位、EFI stub、内存管理
4. **编译时选择**: 零运行时开销的条件编译
5. **渐进式迁移**: 从汇编到 Rust 的平滑过渡

---

## 虚拟化扩展 (LVZ)

### 概述

LoongArch 架构的虚拟化扩展被称为 **LVZ (Loongson Virtualization)**，它是 LoongArch 指令集的五个主要模块之一：

1. **基础指令集** (Loongson Base)
2. **二进制翻译扩展** (LBT)
3. **向量扩展** (LSX - 128位)
4. **高级向量扩展** (LASX - 256位)
5. **虚拟化扩展** (LVZ) ← 当前主题

LVZ 提供了硬件级别的虚拟化支持，使 LoongArch 处理器能够高效运行虚拟机。这一扩展主要在龙芯 3 系列处理器（如 3A6000）中实现。

**发展里程碑**：

- 2023 年 10 月：龙芯宣布为 Linux 内核 6.7 增加 KVM 虚拟化支持
- 2024 年 2 月：Linux 6.7 正式合并 LoongArch KVM 支持
- 2024 年：OpenCloudOS Stream 23 完整支持 LSX、LASX、LVZ 和 LBT 指令集

### CPU 运行模式

实现了 LVZ 虚拟化扩展的处理器支持两个运行模式：

#### Host 模式 (Root Mode)

- 由 Hypervisor（虚拟机监控器）使用
- 拥有对硬件的完全控制权
- 负责管理和调度虚拟机
- 在非虚拟化场景下，直接运行操作系统（如 Linux 内核在 PLV0，用户态在 PLV3）
- 可以访问所有 CSR 寄存器和控制寄存器

#### Guest 模式

- 运行客户机操作系统的模式
- 受 Host 模式下 Hypervisor 的控制
- 通过 **hvcl** (Hypercall) 指令可以主动陷入 Host 模式
- 在诸多方面受限，但仍可通过 GCSR 寄存器组管理自己的特权资源
- 某些敏感操作会自动陷入到 Host 模式

**特权级说明**：每个模式（Host/Guest）都有四个特权级（PLV0-PLV3），由 `CSR.CRMD` 寄存器的 `PLV` 字段确定。

### 虚拟化控制寄存器详解

LVZ 扩展引入了一组新的 CSR 寄存器用于控制虚拟化。基于 Linux 内核实现，以下是每个寄存器的详细分析：

#### 1. CSR_GTLBC (0x15) - Guest TLB Control

客户机 TLB 控制寄存器，用于管理客户机的地址转换缓冲。

```c
// 寄存器地址
#define LOONGARCH_CSR_GTLBC 0x15
```

**位字段定义**：

| 位域 | 位宽 | 名称 | 说明 |
|:-----|:----|:-----|:-----|
| [23:16] | 8 | TGID | Guest TLB ID，用于标记 TLB 项属于哪个 Guest |
| [13] | 1 | TOTI | TLB Operation Trap on Invalid |
| [12] | 1 | USETGID | Use TGID，是否使用 TGID 来区分不同 Guest 的 TLB 项 |
| [5:0] | 6 | GMTLBSZ | Guest TLB Size，Guest TLB 的大小（以页为单位） |

**代码示例** (Linux KVM)：

```c
// main.c:316 - 启用 TGID
set_csr_gtlbc(CSR_GTLBC_USETGID);

// switch.S:71-72 - 设置 TGID
csrrd  t1, LOONGARCH_CSR_GSTAT
bstrpick.w t1, t1, CSR_GSTAT_GID_SHIFT_END, CSR_GSTAT_GID_SHIFT
csrrd  t0, LOONGARCH_CSR_GTLBC
bstrins.w t0, t1, CSR_GTLBC_TGID_SHIFT_END, CSR_GTLBC_TGID_SHIFT
csrwr  t0, LOONGARCH_CSR_GTLBC
```

**功能说明**：

- **TGID**: 用于区分不同虚拟机的 TLB 项，避免 TLB 污染
- **USETGID**: 启用后，TLB 操作会使用 TGID 字段来标记和匹配 TLB 项
- **GMTLBSZ**: 控制 Guest 可用的 TLB 大小

#### 2. CSR_GSTAT (0x50) - Guest Status

客户机状态寄存器，指示当前虚拟化状态和客户机 ID。

```c
#define LOONGARCH_CSR_GSTAT 0x50
```

**位字段定义**：

| 位域 | 位宽 | 名称 | 说明 |
|:-----|:----|:-----|:-----|
| [23:16] | 8 | GID | Guest ID，当前运行虚拟机的唯一标识符 |
| [9:4] | 6 | GIDBIT | Guest ID Bit Width，GID 字段的有效位数 |
| [1] | 1 | PVM | Previous VM Mode，上一次运行模式（0=Host, 1=Guest） |
| [0] | 1 | VM | Current VM Mode，当前运行模式（0=Host, 1=Guest） |

**代码示例**：

```c
// switch.S:83-84 - 设置进入 Guest 模式
ori  t0, zero, CSR_GSTAT_PVM
csrxchg t0, t0, LOONGARCH_CSR_GSTAT  // 设置 PVM=1

// switch.S:148-149 - 退出 Guest 模式
ori  t0, zero, CSR_GSTAT_PVM
csrxchg zero, t0, LOONGARCH_CSR_GSTAT  // 清除 PVM

// main.c:385-387 - 读取 GIDBIT 并计算 vpid_mask
vpid_mask = read_csr_gstat();
vpid_mask = (vpid_mask & CSR_GSTAT_GIDBIT) >> CSR_GSTAT_GIDBIT_SHIFT;
if (vpid_mask)
    vpid_mask = GENMASK(vpid_mask - 1, 0);
```

**功能说明**：

- **GID**: 虚拟机的唯一标识符，范围为 0-255（由 8 位宽度决定）
- **GIDBIT**: 指示 GID 字段的有效位数，用于计算 VPID（Virtual Processor ID）掩码
- **PVM**: 指示上一次的运行模式，用于 `ertn` 指令决定返回到哪个模式
- **VM**: 当前运行模式，1 表示在 Guest 模式，0 表示在 Host 模式

#### 3. CSR_GCFG (0x51) - Guest Config

客户机配置寄存器，控制虚拟化的各种行为和拦截策略。

```c
#define LOONGARCH_CSR_GCFG 0x51
```

**位字段定义**：

| 位域 | 位宽 | 名称 | 说明 |
|:-----|:----|:-----|:-----|
| [26:24] | 3 | GPERF | Guest PMU Number，Guest 可访问的性能监控单元数量 |
| [21:20] | 2 | GCI | Guest Cache Invalidate，缓存指令拦截策略 |
| [19:16] | 4 | GCIP | Guest Cache Instruction Policy，缓存指令策略 |
| [15] | 1 | TORU | Trap on Root Unimplemented，Root 模式未实现指令是否陷入 |
| [14] | 1 | TORUP | Trap on Root Unimplement Prev，前一次 TORU 状态 |
| [13] | 1 | TOP | Trap on Privilege，特权指令拦截 |
| [12] | 1 | TOPP | Trap on Privilege Prev，前一次 TOP 状态 |
| [11] | 1 | TOE | Trap on Exception，异常拦截 |
| [10] | 1 | TOEP | Trap on Exception Prev |
| [9] | 1 | TIT | Trap on Timer，定时器拦截 |
| [8] | 1 | TITP | Trap on Timer Prev |

**GCI 字段值**：

| 值 | 名称 | 说明 |
|:---|:-----|:-----|
| 0 | GCI_ALL | 所有缓存操作都陷入 |
| 1 | GCI_HIT | 缓存命中时陷入 |
| 2 | GCI_SECURE | 安全敏感操作陷入 |

**代码示例** (main.c:284-311)：

```c
// 初始化虚拟化配置
unsigned long env, gcfg = 0;
env = read_csr_gcfg();

// 清空所有配置
write_csr_gcfg(0);

// 设置缓存拦截策略
if (env & CSR_GCFG_GCIP_SECURE)
    gcfg |= CSR_GCFG_GCI_SECURE;  // 拦截安全敏感缓存操作

if (env & CSR_GCFG_MATP_ROOT)
    gcfg |= CSR_GCFG_MATC_ROOT;   // Root 控制内存属性

write_csr_gcfg(gcfg);
```

**功能说明**：

- **GPERF**: 控制客户机可以访问多少个性能监控计数器（0-7）
- **GCI/GCIP**: 控制客户机缓存操作的行为，可以选择是否拦截
- **TORU**: 决定 Root 模式下执行未实现指令是否陷入
- **TOP**: 控制特权指令的拦截策略
- **TOE**: 控制异常处理的拦截
- **TIT**: 控制定时器中断的拦截

#### 4. CSR_GINTC (0x52) - Guest Interrupt Control

客户机中断控制寄存器，管理虚拟机中断的注入和状态。

```c
#define LOONGARCH_CSR_GINTC 0x52
```

**位字段定义**：

| 位域 | 位宽 | 名称 | 说明 |
|:-----|:----|:-----|:-----|
| [23:16] | 8 | HC | Host Cause，Host 触发中断的原因 |
| [15:8] | 8 | PIP | Pending IRQ Pending，待处理的中断位图 |
| [7:0] | 8 | VIP | Virtual IRQ Pending，虚拟中断位图 |

**代码示例**：

```c
// 初始化时清空中断控制
write_csr_gintc(0);
```

**功能说明**：

- **HC**: Host 模式触发中断的原因编码
- **PIP**: 待处理的中断位图，每个位对应一个中断源
- **VIP**: 虚拟中断位图，用于向 Guest 注入中断

#### 5. CSR_GCNTC (0x53) - Guest Timer Compensation

客户机定时器补偿寄存器，用于虚拟机的定时器管理。

```c
#define LOONGARCH_CSR_GCNTC 0x53
```

**功能说明**：

- 用于实现虚拟定时器（Virtual Timer）
- 可以设置偏移量，使 Guest 的时间与 Host 不同步
- 支持 Guest 独立的时间管理

#### 6. CSR_TRGP (0x16) - TLB Read Guest Info

TLB 读取 Guest 信息寄存器，用于 TLB 重填时读取 Guest 相关信息。

```c
#define LOONGARCH_CSR_TRGP 0x16
```

**位字段定义**：

| 位域 | 位宽 | 名称 | 说明 |
|:-----|:----|:-----|:-----|
| [23:16] | 8 | RID | Read ID，读取的 ID 信息 |
| [0] | 1 | GTLB | Guest TLB 标志 |

### GCSR 寄存器组详解

GCSR (Guest Control and Status Registers) 是一套独立的寄存器组，专门供 Guest 模式使用。

#### 设计目的

- **隔离性**: Guest 和 Host 使用独立的 CSR 寄存器，避免冲突
- **性能**: 减少 VM Exit（虚拟机退出）次数
- **灵活性**: Hypervisor 可以选择性地拦截某些 GCSR 操作
- **透明性**: Guest 可以像在真实硬件上一样管理特权资源

#### GCSR 访问指令

LoongArch 提供了专门的指令来访问 GCSR：

```c
// 读取 Guest CSR
#define gcsr_read(csr) \
({ \
    register unsigned long __v; \
    __asm__ __volatile__( \
        " gcsrrd %[val], %[reg]\n\t" \
        : [val] "=r" (__v) \
        : [reg] "i" (csr) \
        : "memory"); \
    __v; \
})

// 写入 Guest CSR
#define gcsr_write(v, csr) \
({ \
    register unsigned long __v = v; \
    __asm__ __volatile__ ( \
        " gcsrwr %[val], %[reg]\n\t" \
        : [val] "+r" (__v) \
        : [reg] "i" (csr) \
        : "memory"); \
    __v; \
})

// 交换并修改 Guest CSR
#define gcsr_xchg(v, m, csr) \
({ \
    register unsigned long __v = v; \
    __asm__ __volatile__( \
        " gcsrxchg %[val], %[mask], %[reg]\n\t" \
        : [val] "+r" (__v) \
        : [mask] "r" (m), [reg] "i" (csr) \
        : "memory"); \
    __v; \
})
```

#### GCSR 寄存器分类

基于 Linux KVM 实现 (kvm_csr.h:48-119)，GCSR 寄存器分为以下几类：

**1. 基础控制寄存器**（硬件虚拟化支持）：

```c
// 异常和状态寄存器
LOONGARCH_CSR_CRMD    // 0x0 - Current Mode
LOONGARCH_CSR_PRMD    // 0x1 - Previous Mode
LOONGARCH_CSR_EUEN    // 0x2 - Extension Unit Enable
LOONGARCH_CSR_MISC    // 0x3 - Miscellaneous
LOONGARCH_CSR_ECFG    // 0x4 - Exception Configuration
LOONGARCH_CSR_ESTAT   // 0x5 - Exception Status
LOONGARCH_CSR_ERA     // 0x6 - Exception Return Address
LOONGARCH_CSR_BADV    // 0x7 - Bad Virtual Address
LOONGARCH_CSR_BADI    // 0x8 - Bad Instruction
LOONGARCH_CSR_EENTRY  // 0xc - Exception Entry
```

**2. 内存管理寄存器**：

```c
LOONGARCH_CSR_ASID    // 0x18 - Address Space ID
LOONGARCH_CSR_PGDL    // 0x19 - Page Table Base (Low)
LOONGARCH_CSR_PGDH    // 0x1a - Page Table Base (High)
LOONGARCH_CSR_PGD     // 0x1b - Page Table Base
LOONGARCH_CSR_PWCTL0  // 0x1c - Page Walk Control 0
LOONGARCH_CSR_PWCTL1  // 0x1d - Page Walk Control 1
LOONGARCH_CSR_STLBPGSIZE // 0x1e - STLB Page Size
```

**3. 处理器信息寄存器**：

```c
LOONGARCH_CSR_CPUID   // 0x20 - CPU ID
LOONGARCH_CSR_PRCFG1  // 0x21 - Processor Config 1
LOONGARCH_CSR_PRCFG2  // 0x22 - Processor Config 2
LOONGARCH_CSR_PRCFG3  // 0x23 - Processor Config 3
```

**4. Scratch 寄存器**（内核专用）：

```c
LOONGARCH_CSR_KS0     // 0x30 - Kernel Scratch 0
LOONGARCH_CSR_KS1     // 0x31 - Kernel Scratch 1
// ... KS2-KS7
```

**5. 定时器寄存器**：

```c
LOONGARCH_CSR_TMID    // 0x40 - Timer ID
LOONGARCH_CSR_TCFG    // 0x41 - Timer Config
LOONGARCH_CSR_TVAL    // 0x42 - Timer Value
LOONGARCH_CSR_CNTC    // 0x43 - Counter Offset
```

**6. TLB 管理寄存器**：

```c
LOONGARCH_CSR_TLBIDX      // 0x10 - TLB Index
LOONGARCH_CSR_TLBRENTRY   // 0x11 - TLB Refill Entry
LOONGARCH_CSR_TLBRBADV    // 0x12 - TLB Refill BadV
LOONGARCH_CSR_TLBRERA     // 0x13 - TLB Refill ERA
LOONGARCH_CSR_TLBRSAVE    // 0x14 - TLB Refill Save
LOONGARCH_CSR_TLBRELO0    // 0x15 - TLB Refill EntryLo0
LOONGARCH_CSR_TLBRELO1    // 0x16 - TLB Refill EntryLo1
LOONGARCH_CSR_TLBREHI     // 0x17 - TLB Refill EntryHi
LOONGARCH_CSR_TLBRPRMD    // 0x1f - TLB Refill PRMD
```

**7. 直接映射窗口寄存器**：

```c
LOONGARCH_CSR_DMWIN0  // 0x80 - Direct Mapping Window 0
LOONGARCH_CSR_DMWIN1  // 0x81 - Direct Mapping Window 1
LOONGARCH_CSR_DMWIN2  // 0x82 - Direct Mapping Window 2
LOONGARCH_CSR_DMWIN3  // 0x83 - Direct Mapping Window 3
```

#### GCSR 软件模拟 vs 硬件支持

Linux KVM 根据 CSR 寄存器的硬件支持程度，将 GCSR 分为三类 (main.c:48-195)：

**1. 硬件支持的 GCSR** (HW_GCSR)：

使用 `gcsrrd`/`gcsrwr` 指令直接访问硬件寄存器，包括：

- 所有基础控制寄存器（CRMD, PRMD, EUEN, ECFG, ESTAT, ERA 等）
- 内存管理寄存器（PGDL, PGDH, ASID, PWCTL 等）
- TLB 管理寄存器（TLBIDX, TLBEHI, TLBELO0/1 等）
- 定时器寄存器（TMID, TCFG, TVAL, CNTC）
- Scratch 寄存器（KS0-KS7）

**2. 软件模拟的 GCSR** (SW_GCSR)：

通过软件模拟实现，Guest 访问时会陷入 Host，包括：

- 实现控制寄存器（IMPCTL1, IMPCTL2）
- 机器错误寄存器（MERRCTL, MERRINFO1/2, MERRENTRY, MERRERA, MERRSAVE）
- 调试寄存器（DEBUG, DERA, DESAVE）
- 性能监控寄存器（PERFCTRL0-3, PERFCNTR0-3）
- 调试地址寄存器（DB0-7ADDR/MASK/CTRL/ASID）
- 指断点寄存器（IB0-7ADDR/MASK/CTRL/ASID）

**3. 无效的 GCSR** (INVALID_GCSR)：

不存在的 CSR 寄存器，访问会触发异常。

**标志设置代码**：

```c
static inline void set_gcsr_sw_flag(int csr)
{
    if (csr < CSR_MAX_NUMS)
        gcsr_flag[csr] |= SW_GCSR;  // 标记为软件模拟
}

static inline void set_gcsr_hw_flag(int csr)
{
    if (csr < CSR_MAX_NUMS)
        gcsr_flag[csr] |= HW_GCSR;  // 标记为硬件支持
}
```

### 虚拟化异常

LVZ 定义了以下虚拟化相关的异常：

| 异常码 | 子码 | 缩写 | 触发原因 |
| :--- | :--- | :--- | :--- |
| 22 | - | **GSPR** | 客户机敏感特权资源异常，由 `cpucfg`、`idle`、`cacop` 指令触发，或访问不存在的 GCSR/IOCSR 时触发 |
| 23 | - | **HVC** | Hypercall 超级调用，由 `hvcl` 指令触发，主动陷入 Hypervisor |
| 24 | 0 | **GCM** | 客户机 GCSR 软件修改异常 |
| 24 | 1 | **GCHC** | 客户机 GCSR 硬件修改异常 |

### 模式切换流程

#### 进入 Guest 模式 (switch_to_guest)

基于 Linux KVM 实现，进入 Guest 模式的步骤如下：

1. **清空异常向量分离**：设置 `CSR.ECFG.VS = 0`（所有异常共用一个入口地址）
2. **加载客户机异常入口**：从 Hypervisor 读取 guest eentry → 写入 `CSR.EENTRY`
3. **加载客户机返回地址**：从 Hypervisor 读取 guest era (GPC) → 写入 `CSR.ERA`
4. **保存 Host 页表**：读取 `CSR.PGDL` 并保存到 Hypervisor
5. **加载 Guest 页表**：从 Hypervisor 加载 guest pgdl → `CSR.PGDL`
6. **设置客户机 ID**：读取 `CSR.GSTAT.GID` 和 `CSR.GTLBC.TGID` → 写入 `CSR.GTLBC`
7. **开启 Host 中断**：设置 `CSR.PRMD.PIE = 1`
8. **设置进入 Guest 模式**：设置 `CSR.GSTAT.PGM = 1`（使 `ertn` 指令进入 guest mode）
9. **恢复客户机寄存器**：将 Hypervisor 中保存的客户机通用寄存器（GPRS）恢复到硬件寄存器
10. **执行 `ertn` 指令**：正式进入 Guest 模式

#### 处理 Guest 异常 (kvm_exc_entry)

当 Guest 模式下发生异常时，处理流程如下：

1. **保存客户机现场**：保存 Guest 的通用寄存器（GPRS）
2. **保存状态寄存器**：
   - `CSR.ESTAT` → host ESTAT
   - `CSR.ERA` → GPC (Guest PC)
   - `CSR.BADV` → host BADV（出错虚地址）
   - `CSR.BADI` → host BADI（出错指令）
3. **恢复 Host 配置**：
   - 写入 Host `ECFG` → `CSR.ECFG`
   - 写入 Host `EENTRY` → `CSR.EENTRY`
   - 写入 Host `PGD` → `CSR.PGDL`
4. **关闭 Guest 模式**：清零 `CSR.GSTAT.PGM`
5. **清空客户机 ID**：清空 `GTLBC.TGID` 域
6. **恢复 KVM per-cpu 寄存器**
7. **跳转到异常处理**：跳转到 `KVM_ARCH_HANDLE_EXIT` 处理具体异常
8. **判断继续运行**：
   - 若返回值 ≤ 0：继续运行 Host
   - 若返回值 > 0：准备再次进入 Guest（保存 percpu 寄存器到 `CSR.KSAVE`）
9. **跳转到 `switch_to_guest`**

### vCPU 上下文切换

根据 LoongArch 函数调用规范，vCPU 上下文切换需要保存的寄存器包括：

**通用寄存器**：

- `$s0` - `$s8`：静态寄存器（被调用者保存）
- `$s9` (`$fp`)：栈帧指针 / 静态寄存器
- `$sp` (`$r3`)：栈指针
- `$ra` (`$r1`)：返回地址

**浮点寄存器**（如果使用）：

- `$fs0` - `$fs7`：静态浮点寄存器（被调用者保存）

### 世界切换流程详解

基于 Linux KVM 源码分析，以下是详细的世界切换流程。

#### kvm_vcpu_arch 结构体

vCPU 的架构相关状态保存在以下结构体中 (kvm_host.h:169-254)：

```c
struct kvm_vcpu_arch {
    // 异常入口地址（存储为 unsigned long 便于汇编直接加载）
    unsigned long host_eentry;    // Host 异常入口
    unsigned long guest_eentry;   // Guest 异常入口

    // Hypervisor 页表基址
    unsigned long kvm_pgd;

    // Host 寄存器保存区
    unsigned long host_sp;        // Host 栈指针
    unsigned long host_tp;        // Host 线程指针
    unsigned long host_pgd;       // Host 页表
    unsigned long host_ecfg;      // Host ECFG
    unsigned long host_estat;     // Host ESTAT
    unsigned long host_percpu;    // Host per-cpu

    // Guest 通用寄存器
    unsigned long gprs[32];       // Guest GPRs[0-31]
    unsigned long pc;             // Guest PC

    // CSR 寄存器状态
    struct loongarch_csrs *csr;   // Guest CSR 数组

    // 中断和异常状态
    unsigned long irq_pending;
    unsigned long exception_pending;
    // ... 其他字段
};
```

**内存布局（关键偏移量）**：

```text
偏移量    字段                说明
0         host_eentry         Host 异常入口地址
8         guest_eentry        Guest 异常入口地址
24        kvm_pgd             Hypervisor 页表基址
32        host_sp             Host 栈指针
40        host_tp             Host 线程指针
48        host_pgd            Host 页表基址
56        host_ecfg           Host ECFG 寄存器
64        host_estat          Host ESTAT 寄存器
72        host_percpu         Host per-cpu 变量
80        gprs[0-31]          Guest 通用寄存器数组（256字节）
336       pc                  Guest 程序计数器
```

#### Host → Guest 切换

**入口**: `kvm_enter_guest` (switch.S:203-219)

**完整流程**：

```text
1. 保存 Host GPRs
   - 保存 $r1($ra), $r2($tp), $r3($sp), $r22-$r31
   - 位置：栈顶分配的 PT_SIZE 区域

2. 保存 Host 特殊寄存器
   - host_sp = 当前 SP
   - host_tp = 当前 TP（线程指针）
   - host_percpu = 当前 per-cpu 变量

3. 保存 vCPU 指针到 kscratch
   - csrwr a1, KVM_VCPU_KS
   - 方便异常入口快速访问 vCPU

4. kvm_switch_to_guest 宏 (switch.S:49-92)
   a) 清空异常向量分离
      - CSR.ECFG.VS = 0（所有异常共用入口）

   b) 加载 Guest 异常入口
      - CSR.EENTRY = vcpu->arch.guest_eentry

   c) 设置 Guest PC
      - CSR.ERA = vcpu->arch.pc  ← 关键步骤！

   d) 加载 Hypervisor 页表
      - CSR.PGDL = vcpu->arch.kvm_pgd

   e) 设置 Guest ID
      - CSR.GTLBC.TGID = CSR.GSTAT.GID

   f) 开启 Host 中断
      - CSR.PRMD.PIE = 1

   g) 设置 PVM 位（关键！）
      - CSR.GSTAT.PVM = 1
      - 告诉硬件 ertn 应进入 Guest 模式

   h) 恢复 Guest GPRs
      - 从 vcpu->arch.gprs[] 恢复所有 GPR

   i) 执行 ertn
      - 硬件检查 CSR.GSTAT.PVM=1
      - 切换到 Guest 模式
      - PC = CSR.ERA（Guest PC）
      - SP = $r3（从 gprs[3] 恢复）
```

**PC 控制机制**：

```assembly
# switch.S:59-61
ld.d    t0, a2, KVM_ARCH_GPC        # t0 = vcpu->arch.pc
csrwr   t0, LOONGARCH_CSR_ERA      # CSR.ERA = Guest PC
ertn                               # 从 ERA 恢复 PC，进入 Guest
```

**SP 控制机制**：

```assembly
# SP 作为通用寄存器 $r3 保存和恢复
# 在 kvm_restore_guest_gprs 中：
ld.d    $r3, a2, (KVM_ARCH_GGPR + 8 * 3)  # 恢复 Guest SP
```

#### Guest → Host 切换

**入口**: `kvm_exc_entry` (switch.S:106-194)

**触发条件**：

- Guest 执行敏感指令
- 中断注入
- 定时器超时
- 访问未实现的功能

**完整流程**：

```text
1. 快速保存现场
   a2 -> KVM_TEMP_KS (临时保存)
   a2 = vcpu 指针 (从 KVM_VCPU_KS 读取)

2. 保存 Guest GPRs
   - 所有 GPR ($r1-$r31) -> vcpu->arch.gprs[]
   - 包括 $a2 (从 KVM_TEMP_KS 恢复)

3. 保存异常状态
   - CSR.ESTAT -> vcpu->arch.host_estat
   - CSR.ERA    -> vcpu->arch.pc      ← 保存 Guest PC
   - CSR.BADV   -> vcpu->arch.badv
   - CSR.BADI   -> vcpu->arch.badi

4. 恢复 Host 配置
   - CSR.ECFG   |= vcpu->arch.host_ecfg
   - CSR.EENTRY = vcpu->arch.host_eentry
   - CSR.PGDL   = vcpu->arch.host_pgd

5. 清除 Guest 模式标志
   - CSR.GSTAT.PVM = 0  ← 确保 ertn 不会误入 Guest
   - CSR.GTLBC.TGID = 0 ← TLB 操作针对 Root 模式

6. 恢复 Host 特殊寄存器
   - $tp  = vcpu->arch.host_tp
   - $sp  = vcpu->arch.host_sp
   - $u0  = vcpu->arch.host_percpu

7. 调用异常处理函数
   - handle_exit(run, vcpu)

8. 判断是否返回 Guest
   - 返回值 ≤ 0: 返回 Host
   - 返回值 > 0: 重新进入 Guest
```

#### 进入 Guest 前的检查

**入口**: `kvm_pre_enter_guest` (vcpu.c:265-306)

**关键步骤**：

```c
1. kvm_enter_guest_check(vcpu)
   - 检查 vcpu 状态

2. local_irq_disable()
   - 关中断，准备切换

3. kvm_deliver_intr(vcpu)
   - 将待处理中断注入到 Guest
   - 修改 GCSR.ESTAT 寄存器

4. kvm_deliver_exception(vcpu)
   - 注入异常到 Guest

5. kvm_check_vpid(vcpu)
   - 检查 VPID (Virtual Processor ID)
   - 如果切换了 CPU，刷新 TLB

6. kvm_check_pmu(vcpu)
   - 如果 Guest 使用 PMU，恢复 PMU 状态
```

### 寄存器保存和控制总结

#### Host 侧需要保存的寄存器

**通用寄存器** (kvm_save_host_gpr):

```assembly
# 只保存调用者保存的寄存器
$r1 ($ra)   - 返回地址
$r2 ($tp)   - 线程指针
$r3 ($sp)   - 栈指针
$r22-$r31   - 静态寄存器 ($s0-$s9)
```

**特殊寄存器**:

```c
vcpu->arch.host_sp      # Host 栈指针
vcpu->arch.host_tp      # Host 线程指针
vcpu->arch.host_pgd     # Host 页表基址
vcpu->arch.host_ecfg    # Host ECFG 寄存器
vcpu->arch.host_estat   # Host ESTAT 寄存器
vcpu->arch.host_percpu  # Host per-cpu 变量
vcpu->arch.host_eentry  # Host 异常入口
```

#### Guest 侧需要保存的寄存器

**通用寄存器**:

```c
vcpu->arch.gprs[0-31]   # 所有 GPR
vcpu->arch.pc           # 程序计数器
```

**CSR 寄存器** (通过 GCSR 指令访问):

```c
// 硬件支持的 CSR
vcpu->arch.csr->csrs[LOONGARCH_CSR_CRMD]    // 当前模式
vcpu->arch.csr->csrs[LOONGARCH_CSR_PRMD]    // 前一个模式
vcpu->arch.csr->csrs[LOONGARCH_CSR_EUEN]    // 扩展单元使能
vcpu->arch.csr->csrs[LOONGARCH_CSR_ECFG]    // 异常配置
vcpu->arch.csr->csrs[LOONGARCH_CSR_ESTAT]   // 异常状态
vcpu->arch.csr->csrs[LOONGARCH_CSR_PGDL]    // 页表基址
// ... 其他 CSR
```

#### PC 和 SP 的控制机制

**PC (程序计数器)**:

```text
1. 设置阶段
   vcpu->arch.pc = guest_pc_value  (用户态通过 KVM_SET_REGS)

2. 切换阶段
   ld.d t0, a2, KVM_ARCH_GPC       # t0 = vcpu->arch.pc
   csrwr t0, LOONGARCH_CSR_ERA     # CSR.ERA = Guest PC

3. 进入 Guest
   ertn                             # 硬件从 ERA 恢复 PC

4. 退出阶段
   csrrd t0, LOONGARCH_CSR_ERA     # 读取当前 PC
   st.d t0, a2, KVM_ARCH_GPC       # 保存到 vcpu->arch.pc
```

**SP (栈指针)**:

```text
1. 设置阶段
   vcpu->arch.gprs[3] = guest_sp_value

2. 切换阶段
   kvm_restore_guest_gprs a2
   -> ld.d $r3, a2, (KVM_ARCH_GGPR + 8 * 3)  # 恢复 $sp

3. Guest 执行
   Guest 使用恢复的 $sp

4. 退出阶段
   kvm_save_guest_gprs a2
   -> st.d $r3, a2, (KVM_ARCH_GGPR + 8 * 3)  # 保存 $sp
```

**关键差异**：

- **PC**: 通过特殊的 CSR 寄存器 (`ERA`) 控制，利用异常返回机制
- **SP**: 作为通用寄存器的一部分处理

### 技术参考

**官方文档**：

- [龙芯架构参考手册 卷三：虚拟化扩展](https://loongson.github.io/LoongArch-Documentation/)

**开源实现**：

- [Linux KVM LoongArch 源码](https://github.com/torvalds/linux/blob/master/arch/loongarch/kvm/)
  - [switch.S - 世界切换汇编代码](https://github.com/torvalds/linux/blob/master/arch/loongarch/kvm/switch.S)
  - [vcpu.c - vCPU 管理](https://github.com/torvalds/linux/blob/master/arch/loongarch/kvm/vcpu.c)
  - [main.c - 虚拟化初始化](https://github.com/torvalds/linux/blob/master/arch/loongarch/kvm/main.c)
  - [kvm_host.h - Hypervisor 数据结构](https://github.com/torvalds/linux/blob/master/arch/loongarch/include/asm/kvm_host.h)
  - [kvm_csr.h - CSR 寄存器操作](https://github.com/torvalds/linux/blob/master/arch/loongarch/include/asm/kvm_csr.h)
- [hvisor 虚拟化文档](https://hvisor.syswonder.org/chap04/subchap01/LoongArchVirtualization.html)

**相关资源**：

- [龙芯 KVM 虚拟化官方页面](https://www.loongnix.cn/zh/cloud/kvm/)
- [在 QEMU 上调试 Loongson 内核](https://utopianfuture.github.io/kernel/debug-loongarch-kernel-in-qemu.html)

