# Linux Guest Kernel for AxVisor

Target: Linux 6.12.94, aarch64, built as a flat `Image` for AxVisor Type-1 hypervisor.

## Files

| File | Description |
|------|-------------|
| `config` | Kernel `.config` (Alpine virt baseline + debug options) |
| `panic_hvc.patch` | HVC #13 (GuestPanic) patch for crash monitoring |
| `build.sh` | Build script (download, patch, compile, copy) |

---

## 1. 架构概览

```
┌─────────────────────────────────────────────────────┐
│  QEMU (aarch64, EL2)                                │
│  ┌───────────────────────────────────────────────┐  │
│  │  AxVisor (Type-1 Hypervisor, EL2)             │  │
│  │                                               │  │
│  │  ┌──────────────┐   ┌──────────────────────┐  │  │
│  │  │ VM[1]: Linux │   │ VM[2]: Monitor Guest │  │  │
│  │  │  (Target)    │   │  (Crash Capture)     │  │  │
│  │  │              │   │                      │  │  │
│  │  │ panic() ──HVC#13──▶ GuestPanic          │  │  │
│  │  │              │   │  ◀──HVC#10 Poll────  │  │  │
│  │  │              │   │  ◀──HVC#9  ReadMem── │  │  │
│  │  └──────────────┘   └──────────────────────┘  │  │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

Linux 作为 **VM[1] (Target Guest)** 运行在 AxVisor 之上。崩溃时通过 HVC #13 通知 Hypervisor，Monitor Guest (VM[2]) 轮询检测崩溃状态后通过 HVC #9 读取 VM[1] 内存进行 crash dump。

## 2. 内存布局

```
GPA 范围                  大小    用途
────────────────────────────────────────────────
0x8000_0000 - 0x9000_0000  256MB   VM RAM (MAP_IDENTICAL)
  0x8020_0000                     kernel Image 入口
0xA000_0000 - 0xA000_4000  16KB   virtio-mmio 设备 (passthrough)
```

关键符号地址 (VA → PA)：

| 符号 | VA | PA | 用途 |
|------|----|----|------|
| `_text` | `0xffff800080000000` | `0x80000000` | 内核代码段起始 |
| `printk_rb_static` | `0xffff800081298108` | `0x81298108` | prb ringbuffer 结构体 |
| `__log_buf` | `0xffff8000814d8000` | `0x814d8000` | printk 数据环缓冲区 |

> **注意**: VA → PA 转换公式：
> - 内核镜像区 (0xffff8000_xxxxxxxx): `PA = VA - 0xffff800080000000 + 0x80000000`
> - 线性映射区 (0xffff0000_xxxxxxxx): `PA = VA - 0xffff000000000000`

## 3. 构建步骤

### 3.1 下载内核源码

```bash
git clone --depth 1 --branch v6.12.94 \
  https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git \
  /tmp/linux-6.12.94
```

### 3.2 安装交叉编译器

```bash
apt install gcc-aarch64-linux-gnu
```

### 3.3 编译内核

```bash
cd tgoskits/os/linux
./build.sh
# 输出: tgoskits/target/linux-Image-6.12.94
```

`build.sh` 自动完成：应用 `.config` → 打 `panic_hvc.patch` → 编译 → 复制 Image。

### 3.4 准备 rootfs

使用 Alpine Linux aarch64 根文件系统：

```bash
# 下载 Alpine minirootfs
wget https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/aarch64/alpine-minirootfs-3.21.0-aarch64.tar.gz

# 创建 1GB 镜像
dd if=/dev/zero of=tmp/axbuild/rootfs/rootfs-aarch64-alpine.img bs=1M count=1024
mkfs.ext4 tmp/axbuild/rootfs/rootfs-aarch64-alpine.img

# 挂载并解压根文件系统
mkdir -p /mnt/rootfs
mount tmp/axbuild/rootfs/rootfs-aarch64-alpine.img /mnt/rootfs
tar xzf alpine-minirootfs-3.21.0-aarch64.tar.gz -C /mnt/rootfs
umount /mnt/rootfs
```

### 3.5 部署内核模块到 rootfs

模块通过 `debugfs` 写入 rootfs 镜像（无需挂载）：

```bash
# 编译模块
cd /tmp/linux-6.12.94
make modules_prepare ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-
cd /path/to/module
make -C /tmp/linux-6.12.94 M=$PWD ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-

# 写入 rootfs
printf "rm /module.ko\nwrite /path/to/module.ko /module.ko\nquit\n" \
  | debugfs -w tmp/axbuild/rootfs/rootfs-aarch64-alpine.img

# 修复文件系统（debugfs 直接写可能破坏元数据）
e2fsck -fy tmp/axbuild/rootfs/rootfs-aarch64-alpine.img
```

> **注意**: 每次用 `debugfs -w` 写文件后必须运行 `e2fsck -fy`，否则 Linux guest 中 `insmod` 会报 "deleted inode referenced"。

## 4. VM 配置文件

Linux guest 的 VM 配置在 `os/axvisor/configs/vms/qemu/aarch64/linux-smp1.toml`。

关键配置项：

```toml
[base]
id = 1                          # VM ID，Monitor Guest 用此 ID 轮询
name = "linux-qemu"
vm_type = 1                     # 1 = 普通 VM
cpu_num = 1

[kernel]
entry_point = 0x8020_0000       # Image 入口地址
image_location = "memory"       # 内核嵌入到 axvisor.bin 中（不是从文件系统加载）
kernel_path = "../../../../../../target/linux-Image-6.12.94"
kernel_load_addr = 0x8020_0000  # 加载到 VM 内存的地址

memory_regions = [
  [0x8000_0000, 0x1000_0000, 0x7, 1],  # 256MB RAM, RW, MAP_IDENTICAL
]

[devices]
interrupt_mode = "passthrough"  # 直通中断
passthrough_devices = [["/"]]   # 直通所有设备
passthrough_addresses = [
  { base_gpa = 0xA000000, length = 0x4000 },  # virtio-mmio 区域
]
```

### 运行双 VM

```bash
# QEMU 配置指定两个 VM: Linux (VM[1]) + Monitor (VM[2])
cargo xtask axvisor qemu --arch aarch64
```

这会使用 `qemu-aarch64.toml` 中的 QEMU 参数（包括 `-drive` 挂载 rootfs），并启动两个 VM。

## 5. Linux Guest 中的操作

启动后进入 `/bin/sh`：

```bash
# 挂载伪文件系统
mount -t proc proc /proc
mount -t sysfs sysfs /sys

# 配置崩溃行为
echo 1 > /proc/sys/kernel/panic_on_oops   # Oops 也触发 panic
echo 0 > /proc/sys/kernel/panic           # 禁用自动重启

# 触发崩溃的几种方式
echo c > /proc/sysrq-trigger              # 方式1: SysRq panic
insmod /crash_test.ko action=null         # 方式2: NULL 指针 (Data Abort)
insmod /crash_test.ko action=bug          # 方式3: BUG()
insmod /crash_test.ko action=undefined    # 方式4: 未定义指令
```

## 6. 崩溃监控链路

```
Linux panic()
  └─ panic_hvc.patch: HVC #13 (x0=13) ──▶ AxVisor hvc.rs: GuestPanic
                                            └─ vm.shutdown() → state=Stopping
                                               └─ vcpu loop: Stopping→Stopped

Monitor Guest (每 2s 轮询)
  └─ HVC #10 (PollCrashStatus, VM[1]) ──▶ AxVisor: Stopped → return Ok
     └─ capture snapshot: 内存 dump + dmesg + 寄存器
        └─ 分析报告 + 交互式控制台
```

## 7. dmesg 捕获原理

Linux 6.12 使用 prb (printk ringbuffer) 存储内核日志，格式为：

```
desc_ring.infos[]   → printk_info (含 text_len, seq, timestamp)
desc_ring.descs[]   → prb_desc (含 state_var, text_blk_lpos)
text_data_ring.data → __log_buf: [u64 id][text bytes] 数据块
```

Monitor Guest 的 `log.rs` 通过 HVC #9 读取 `printk_rb_static` 结构体，遍历 finalized 描述符，提取文本数据。

## 8. 关键内核配置选项

| Option | Value | Purpose |
|--------|-------|---------|
| `CONFIG_MODULES=y` | enabled | 加载 crash test 内核模块 |
| `CONFIG_KPROBES=y` | enabled | 动态插桩 |
| `CONFIG_DEBUG_FS=y` | enabled | 调试文件系统 |
| `CONFIG_MAGIC_SYSRQ=y` | enabled | SysRq 崩溃触发 (`echo c`) |
| `CONFIG_KALLSYMS=y` | enabled | 符号表 (地址解析) |
| `CONFIG_PRINTK=y` | enabled | 内核日志 (prb ringbuffer) |
| `CONFIG_LOG_BUF_SHIFT=17` | 128KB | 日志缓冲区大小 |
| `CONFIG_PANIC_ON_OOPS=y` | enabled | Oops 触发 panic |
| `CONFIG_DEBUG_INFO=y` | enabled | DWARF 调试信息 |
| `CONFIG_ARM64_VA_BITS=48` | 48-bit | 虚拟地址宽度 |
