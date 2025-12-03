[根目录](../../CLAUDE.md) > [crates](../) > **somehal**

# SomeHAL - 跨平台硬件抽象层

## 模块职责

SomeHAL 是 Sparreal OS 的跨平台硬件抽象层，提供统一的硬件接口，支持 AArch64 和 LoongArch64 两种架构。它屏蔽了底层硬件差异，让内核可以在不同架构上运行相同的代码。

## 架构支持

### 支持的架构
- **AArch64**: ARMv8-A 64位架构
  - EL1 (Exception Level 1) - Linux 内核级别
  - EL2 (Exception Level 2) - 虚拟化支持 (可选)
- **LoongArch64**: 龙芯64位架构
  - 完整的页表管理
  - 中断和异常处理
  - 缓存管理

### 架构抽象接口
所有架构都实现 `ArchTrait`，提供统一的接口：
- 内存管理 (`virt_to_phys`, `phys_to_virt`, `ioremap`)
- 定时器管理 (`systimer_enable`, `systimer_set_interval`)
- 中断管理 (`irq_set_enable`, `irq_is_enabled`)
- 页表管理 (`create_page_table`, `set_kernel_page_table`)

## 入口与启动

### 架构入口点
每个架构都有专门的启动流程：

**AArch64** (`arch/aarch64/entry.rs`):
```rust
#[unsafe(naked)]
pub unsafe extern "C" fn kernel_entry(_fdt_addr: usize) -> !
```
- 清零 BSS 段
- 设置栈指针
- 切换到目标异常级别
- 调用 `el_entry()` 进行硬件初始化

**LoongArch64** (`arch/loongarch64/entry.rs`):
```rust
#[unsafe(naked)]
pub unsafe extern "C" fn kernel_entry(
    efi_boot: usize,
    cmdline: *const u8,
    systemtable: *const c_void,
) -> !
```
- 设置直接映射窗口 (DMW0/DMW1/DMW2)
- 跳转到虚拟地址空间
- 启用分页机制
- 清零 BSS 段

## 对外接口

### 核心抽象层 (`al/`)
- **`mod.rs`**: 统一的抽象接口导出
- **`console.rs`**: 控制台抽象 (`early_write`, `early_read`)
- **`memory.rs`**: 内存管理抽象
  - 地址转换: `virt_to_phys()`, `phys_to_virt()`
  - 页大小: `page_size()`
  - 内存映射: `memory_map()`
- **`platform.rs`**: 平台特定功能
  - 中断控制: `irq_set_enabled()`
  - 系统定时器: `systimer_enable()`
- **`timer.rs`**: 定时器管理
  - `enable()`, `set_next_event()`, `ack()`

### 架构特定实现

#### AArch64 (`arch/aarch64/`)
- **`mod.rs`**: `Arch` 结构体实现 `ArchTrait`
- **`paging.rs`**: `enable_mmu()` - 启用内存管理单元
- **`trap.rs`**: 异常处理，使用 `kasm_aarch64` 宏
  - 支持同步异常、IRQ、FIQ、SError
- **`el1/mod.rs`**: EL1 特定实现 (标准内核级别)
- **`el2/mod.rs`**: EL2 特定实现 (虚拟化支持)

#### LoongArch64 (`arch/loongarch64/`)
- **`mod.rs`**: `Arch` 结构体实现 `ArchTrait`
- **`paging.rs`**: 页表管理
  - CSR 寄存器操作: `read_csr_pgdh()`, `write_csr_pgdh()`
  - TLB 管理: `local_flush_tlb_all()`
- **`trap.rs`**: 异常和中断处理
  - 异常码定义 (TLBL, TLBS, TLBI 等)
  - 异常分发处理
- **`addrspace.rs`**: 地址空间管理
- **`cache.rs`**: 缓存操作
- **`register/`**: CSR 寄存器定义和操作

### 页表管理 (`mem/`)
- **通用接口**:
  - `new_page_table<T>()` - 创建新页表
  - `kernel_page_table_paddr()` - 获取内核页表物理地址
  - `set_kernel_page_table_paddr()` - 设置内核页表
- **页表项抽象**: `Pte` trait
  - 地址设置: `set_paddr()`, `paddr()`
  - 属性控制: `set_valid()`, `set_mem_config()`
  - 大页支持: `set_is_huge()`, `is_huge()`

## 关键数据模型

### 地址抽象
- **物理地址**: `PhysAddr` - 跨架构的物理地址表示
- **虚拟地址**: `VirtAddr` - 跨架构的虚拟地址表示
- **页表项**: `Pte` trait - 统一的页表项接口

### 中断抽象
- **中断ID**: `IrqId` - 中断标识符
- **中断类型**: 私有中断、外部中断

### 内存描述符
- **`MemoryDescriptor`**: 内存区域描述
  - 物理起始地址、大小、内存类型
  - 支持可使用内存、保留内存等类型

### 页表配置
- **`MemConfig`**: 内存映射配置
  - 访问权限 (`AccessFlags`)
  - 内存属性 (`MemAttributes`)

## 特殊功能

### 控制台支持
- 早期控制台输出，支持启动时调试信息
- 架构特定的串口初始化

### EFI 支持 (LoongArch64)
- 支持 EFI 引导环境
- 获取启动参数和命令行

### 设备树支持 (AArch64)
- FDT (Flattened Device Tree) 解析
- 早期控制台初始化

## 测试与质量

### 当前测试状态
- ✅ **架构兼容性**: 在 QEMU 中验证 AArch64 和 LoongArch64
- ✅ **启动流程**: 完整的内核启动到用户空间
- ✅ **内存管理**: 分页机制正确工作
- ⚠️ **单元测试**: 缺少详细的架构特定测试

### 质量保证
- **内联汇编**: 使用 `naked_asm!` 确保性能
- **安全性**: 大量使用 `unsafe` 代码，需要仔细审查
- **跨架构一致性**: 通过 `ArchTrait` 确保接口一致性

## 常见问题 (FAQ)

### Q: 如何添加新架构支持？
A: 创建新的 `arch/<arch>/` 目录，实现 `ArchTrait`，并添加相应的启动代码。

### Q: AArch64 的 EL1 和 EL2 有什么区别？
A: EL1 是标准内核级别，EL2 支持虚拟化。通过 `hv` feature 选择。

### Q: LoongArch64 的 DMW 是什么？
A: 直接映射窗口 (Direct Mapped Window)，提供物理地址到虚拟地址的直接映射。

### Q: 页表项大小如何配置？
A: 每个架构在 `TableGeneric` 实现中定义自己的 `PAGE_SIZE` 和 `LEVEL_BITS`。

## 相关文件清单

### 核心文件
- `src/lib.rs` - 库入口和公共接口
- `src/consts.rs` - 常量定义

### 抽象层
- `src/al/mod.rs` - 抽象层主模块
- `src/al/console.rs` - 控制台抽象
- `src/al/memory.rs` - 内存管理抽象
- `src/al/platform.rs` - 平台特定抽象
- `src/al/timer.rs` - 定时器抽象

### AArch64 架构
- `src/arch/aarch64/mod.rs` - AArch64 主模块
- `src/arch/aarch64/entry.rs` - 启动入口
- `src/arch/aarch64/head.rs` - 启动代码
- `src/arch/aarch64/paging.rs` - 页表管理
- `src/arch/aarch64/trap.rs` - 异常处理
- `src/arch/aarch64/el1/mod.rs` - EL1 实现
- `src/arch/aarch64/el2/mod.rs` - EL2 实现
- `src/arch/aarch64/context.rs` - 上下文管理
- `src/arch/aarch64/relocate.rs` - 重定位
- `src/arch/aarch64/vectors.s` - 中断向量表
- `src/arch/aarch64/link.ld` - 链接脚本

### LoongArch64 架构
- `src/arch/loongarch64/mod.rs` - LoongArch64 主模块
- `src/arch/loongarch64/entry.rs` - 启动入口
- `src/arch/loongarch64/head.rs` - 启动代码
- `src/arch/loongarch64/paging.rs` - 页表管理
- `src/arch/loongarch64/trap.rs` - 异常处理
- `src/arch/loongarch64/addrspace.rs` - 地址空间
- `src/arch/loongarch64/cache.rs` - 缓存管理
- `src/arch/loongarch64/context.rs` - 上下文管理
- `src/arch/loongarch64/relocate.rs` - 重定位
- `src/arch/loongarch64/register/mod.rs` - CSR 寄存器
- `src/arch/loongarch64/link.ld` - 链接脚本

### x86_64 架构 (部分支持)
- `src/arch/x86_64/mod.rs` - x86_64 主模块
- `src/arch/x86_64/paging.rs` - 页表管理

### 构建文件
- `somehal.x` - 链接器脚本模板
- `build.rs` - 构建脚本

---

## 变更记录 (Changelog)

### 2025-12-03 09:30:10
- 完成 somehal 模块深度分析
- 详细记录 AArch64 和 LoongArch64 架构实现
- 分析页表管理、异常处理、启动流程
- 识别架构特性和跨架构抽象机制