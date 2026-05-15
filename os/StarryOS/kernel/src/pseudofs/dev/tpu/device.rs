//! TPU 设备 OS 适配
//!
//! 将 ioctl 命令翻译为 `Sg2002Tpu` 调用，并通过 fd 解析 Ion buffer
//! 物理/虚拟地址。

use alloc::sync::Arc;

use sg2002_tpu::{
    ion::{IonBufferManager, IonHandle},
    tpu::{
        Sg2002Tpu,
        error::TpuError,
        types::{
            CVITPU_DMABUF_FLUSH, CVITPU_DMABUF_FLUSH_FD, CVITPU_DMABUF_INVLD,
            CVITPU_DMABUF_INVLD_FD, CVITPU_LOAD_TEE, CVITPU_PIO_MODE, CVITPU_SUBMIT_DMABUF,
            CVITPU_SUBMIT_TEE, CVITPU_UNLOAD_TEE, CVITPU_WAIT_DMABUF, CviCacheOpArg,
            CviSubmitDmaArg, CviWaitDmaArg,
        },
    },
};

use crate::{
    file::{get_file_like, ion::IonBufferFile},
    pseudofs::{DeviceOps, dev::ion::global_ion_buffer_manager},
};

/// TPU 字符设备
pub struct TpuDevice {
    /// 硬件层
    hw: Sg2002Tpu,
    /// Ion buffer 管理器引用
    ion_manager: Arc<IonBufferManager>,
}

impl TpuDevice {
    /// 创建 TPU 设备（使用默认物理地址）
    ///
    /// # Safety
    /// 调用者必须确保偏移计算后的虚拟地址有效。
    pub unsafe fn new() -> Self {
        Self {
            hw: unsafe { Sg2002Tpu::new() },
            ion_manager: global_ion_buffer_manager(),
        }
    }

    /// 使用指定的虚拟地址创建 TPU 设备
    ///
    /// # Safety
    /// 调用者必须确保虚拟地址有效。
    #[allow(dead_code)]
    pub unsafe fn from_vaddr(tdma_vaddr: *mut u8, tiu_vaddr: *mut u8) -> Self {
        Self {
            hw: unsafe { Sg2002Tpu::from_vaddr(tdma_vaddr, tiu_vaddr) },
            ion_manager: global_ion_buffer_manager(),
        }
    }

    /// 提交 DMA buffer 任务
    fn submit_dmabuf(&self, arg: usize) -> Result<usize, TpuError> {
        // 从用户空间读取参数
        let submit_arg = unsafe { &*(arg as *const CviSubmitDmaArg) };

        debug!(
            "[TPU] submit dmabuf: fd={}, seq_no={}",
            submit_arg.fd, submit_arg.seq_no
        );

        // 从文件描述符获取 IonBufferFile
        let fd = submit_arg.fd;
        let file = get_file_like(fd).map_err(|_| {
            error!("[TPU] Failed to get file for fd={}", fd);
            TpuError::InvalidDmabuf
        })?;

        // 尝试转换为 IonBufferFile (使用 downcast_arc)
        let ion_file: Arc<IonBufferFile> = file.downcast_arc::<IonBufferFile>().map_err(|_| {
            error!("[TPU] fd={} is not an IonBufferFile", fd);
            TpuError::InvalidDmabuf
        })?;

        // 获取缓冲区信息
        let buffer_info = ion_file.info();
        debug!(
            "[TPU] dmabuf info: handle={}, size={}, paddr=0x{:x}",
            buffer_info.handle, buffer_info.size, buffer_info.phys_addr
        );

        // 从 Ion 管理器获取完整的 buffer 信息
        let handle = IonHandle(buffer_info.handle);
        let buffer = self
            .ion_manager
            .get_buffer(handle)
            .map_err(|_| TpuError::InvalidDmabuf)?;

        let dmabuf_vaddr = buffer.dma_info.cpu_addr.as_ptr() as usize;
        let dmabuf_paddr = buffer.dma_info.bus_addr.as_u64();

        self.hw
            .submit_dmabuf(submit_arg.fd, submit_arg.seq_no, dmabuf_vaddr, dmabuf_paddr)?;

        Ok(0)
    }

    /// 等待 DMA buffer 完成
    fn wait_dmabuf(&self, arg: usize) -> Result<usize, TpuError> {
        let wait_arg = unsafe { &mut *(arg as *mut CviWaitDmaArg) };

        match self.hw.wait_dmabuf(wait_arg.seq_no) {
            Ok(ret) => {
                wait_arg.ret = ret;
                Ok(0)
            }
            Err(e) => {
                wait_arg.ret = -1;
                Err(e)
            }
        }
    }

    /// 刷新 DMA buffer 缓存 (通过物理地址)
    fn cache_flush(&self, arg: usize) -> Result<usize, TpuError> {
        let flush_arg = unsafe { &*(arg as *const CviCacheOpArg) };
        self.hw.cache_flush_paddr(flush_arg.paddr, flush_arg.size)?;
        Ok(0)
    }

    /// 无效化 DMA buffer 缓存 (通过物理地址)
    fn cache_invalidate(&self, arg: usize) -> Result<usize, TpuError> {
        let invalidate_arg = unsafe { &*(arg as *const CviCacheOpArg) };
        self.hw
            .cache_invalidate_paddr(invalidate_arg.paddr, invalidate_arg.size)?;
        Ok(0)
    }

    /// 刷新 DMA buffer 缓存 (通过 fd)
    fn dmabuf_flush_fd(&self, arg: usize) -> Result<usize, TpuError> {
        let fd = arg as i32;
        debug!("TPU dmabuf flush fd: {}", fd);

        let handle = IonHandle(fd as u32);
        if let Ok(buffer) = self.ion_manager.get_buffer(handle) {
            let paddr = buffer.dma_info.bus_addr.as_u64();
            let size = buffer.size as u64;
            self.hw.cache_flush_paddr(paddr, size)?;
            debug!("Flushed buffer: paddr=0x{:x}, size={}", paddr, size);
        }

        Ok(0)
    }

    /// 无效化 DMA buffer 缓存 (通过 fd)
    fn dmabuf_invld_fd(&self, arg: usize) -> Result<usize, TpuError> {
        let fd = arg as i32;
        debug!("TPU dmabuf invalidate fd: {}", fd);

        let handle = IonHandle(fd as u32);
        if let Ok(buffer) = self.ion_manager.get_buffer(handle) {
            let paddr = buffer.dma_info.bus_addr.as_u64();
            let size = buffer.size as u64;
            self.hw.cache_invalidate_paddr(paddr, size)?;
        }

        Ok(0)
    }
}

impl DeviceOps for TpuDevice {
    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> axfs_ng_vfs::VfsResult<usize> {
        Ok(0)
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> axfs_ng_vfs::VfsResult<usize> {
        Ok(0)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> axfs_ng_vfs::VfsResult<usize> {
        debug!("TPU ioctl: cmd=0x{:x}, arg=0x{:x}", cmd, arg);

        let result = match cmd {
            CVITPU_SUBMIT_DMABUF => self.submit_dmabuf(arg),
            CVITPU_DMABUF_FLUSH_FD => self.dmabuf_flush_fd(arg),
            CVITPU_DMABUF_INVLD_FD => self.dmabuf_invld_fd(arg),
            CVITPU_DMABUF_FLUSH => self.cache_flush(arg),
            CVITPU_DMABUF_INVLD => self.cache_invalidate(arg),
            CVITPU_WAIT_DMABUF => self.wait_dmabuf(arg),
            CVITPU_PIO_MODE => {
                warn!("TPU PIO mode not implemented");
                Ok(0)
            }
            CVITPU_LOAD_TEE | CVITPU_SUBMIT_TEE | CVITPU_UNLOAD_TEE => {
                warn!("TPU TEE operations not supported");
                Err(TpuError::NotInitialized)
            }
            _ => {
                warn!("Unknown TPU ioctl command: 0x{:x}", cmd);
                Err(TpuError::NotInitialized)
            }
        };

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                error!("TPU ioctl error: {:?}", e);
                Err(ax_errno::AxError::Unsupported)
            }
        }
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
