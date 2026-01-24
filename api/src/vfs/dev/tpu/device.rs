//! TPU 设备抽象
//!
//! 提供高层 API 供操作系统调用

use super::error::TpuError;
use super::platform::TpuRuntimeState;
use super::tdma::TdmaRegs;
use super::tiu::TiuRegs;
use super::{TDMA_PHYS_BASE, TIU_PHYS_BASE};

use crate::vfs::DeviceOps;

/// TPU 设备状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpuState {
    /// 未初始化
    Uninitialized,
    /// 空闲
    Idle,
    /// 运行中
    Running,
    /// 已挂起
    Suspended,
}

/// TPU 设备
pub struct TpuDevice {
    /// TDMA 寄存器
    tdma: TdmaRegs,
    /// TIU 寄存器
    tiu: TiuRegs,
    /// 设备状态
    state: TpuState,
    /// 运行时状态
    runtime: TpuRuntimeState,
}

impl TpuDevice {
    /// 创建未初始化的 TPU 设备
    /// 
    /// 使用默认物理地址，需要提供虚拟地址偏移
    /// 
    /// # Arguments
    /// * `virt_offset` - 虚拟地址 = 物理地址 + virt_offset
    /// 
    /// # Safety
    /// 调用者必须确保偏移计算后的虚拟地址有效
    pub unsafe fn new() -> Self {
        let virt_offset = 0xffff_ffc0_0000_0000u64 as isize;
        let tdma_vaddr = (TDMA_PHYS_BASE as isize + virt_offset) as *mut u8;
        let tiu_vaddr = (TIU_PHYS_BASE as isize + virt_offset) as *mut u8;

        Self {
            tdma: unsafe { TdmaRegs::new(tdma_vaddr) },
            tiu: unsafe { TiuRegs::new(tiu_vaddr) },
            state: TpuState::Uninitialized,
            runtime: TpuRuntimeState::default(),
        }
    }

    /// 使用指定的虚拟地址创建 TPU 设备
    /// 
    /// # Safety
    /// 调用者必须确保虚拟地址有效
    pub unsafe fn from_vaddr(tdma_vaddr: *mut u8, tiu_vaddr: *mut u8) -> Self {
        Self {
            tdma: unsafe { TdmaRegs::new(tdma_vaddr) },
            tiu: unsafe { TiuRegs::new(tiu_vaddr) },
            state: TpuState::Uninitialized,
            runtime: TpuRuntimeState::default(),
        }
    }

    /// 初始化 TPU 设备 (probe)
    pub fn init(&mut self) -> Result<(), TpuError> {
        // 重置命令 ID
        super::platform::resync_cmd_id(&self.tdma, &self.tiu);
        
        self.state = TpuState::Idle;
        self.runtime = TpuRuntimeState::default();
        
        Ok(())
    }

    /// 获取设备状态
    pub fn state(&self) -> TpuState {
        self.state
    }

    /// 检查设备是否就绪
    pub fn is_ready(&self) -> bool {
        self.state == TpuState::Idle
    }

    /// 检查是否有中断待处理
    pub fn is_irq_pending(&self) -> bool {
        self.runtime.irq_received
    }

    /// 处理 TDMA 中断
    /// 
    /// 应该在你的 OS 中断处理程序中调用此函数
    /// 
    /// 返回是否有错误发生
    pub fn handle_irq(&mut self) -> bool {
        super::platform::handle_tdma_irq(&self.tdma, &self.tiu, &mut self.runtime)
    }

    /// 启动 DMA buffer 执行 (异步)
    /// 
    /// 启动后需要等待中断，然后调用 `complete_run` 检查结果
    pub fn start_run(
        &mut self,
        dmabuf_vaddr: *const u8,
        dmabuf_paddr: u64,
    ) -> Result<(), TpuError> {
        if self.state != TpuState::Idle {
            return Err(TpuError::NotInitialized);
        }

        self.state = TpuState::Running;
        self.runtime.irq_received = false;

        // 解析 header
        let header = unsafe { &*(dmabuf_vaddr as *const super::types::DmaHeader) };

        if !header.is_valid() {
            self.state = TpuState::Idle;
            return Err(TpuError::InvalidDmabuf);
        }

        if (dmabuf_paddr & 0xFFF) != 0 {
            self.state = TpuState::Idle;
            return Err(TpuError::DmabufNotAligned);
        }

        // 设置 array base
        self.tdma.set_array_bases(header);

        Ok(())
    }

    /// 同步执行 DMA buffer
    /// 
    /// 这是主要的执行接口，会阻塞直到完成或超时
    /// 
    /// # Arguments
    /// * `dmabuf_vaddr` - DMA buffer 虚拟地址
    /// * `dmabuf_paddr` - DMA buffer 物理地址
    /// * `wait_irq` - 等待中断的回调函数
    /// * `timeout_checker` - 超时检查回调，返回 true 表示超时
    pub fn run_dmabuf<F, T>(
        &mut self,
        dmabuf_vaddr: *const u8,
        dmabuf_paddr: u64,
        wait_irq: F,
        timeout_checker: T,
    ) -> Result<(), TpuError>
    where
        F: Fn() -> Result<(), TpuError>,
        T: Fn() -> bool,
    {
        if self.state != TpuState::Idle {
            return Err(TpuError::NotInitialized);
        }

        self.state = TpuState::Running;

        let result = super::platform::run_dmabuf(
            &self.tdma,
            &self.tiu,
            dmabuf_vaddr,
            dmabuf_paddr,
            &mut self.runtime,
            wait_irq,
            timeout_checker,
        );

        self.state = TpuState::Idle;
        result
    }

    /// 挂起 TPU
    pub fn suspend(&mut self) -> Result<(), TpuError> {
        if self.state == TpuState::Suspended {
            return Ok(());
        }

        super::platform::backup_registers(&self.tdma, &self.tiu, &mut self.runtime.reg_backup);
        self.state = TpuState::Suspended;
        
        Ok(())
    }

    /// 恢复 TPU
    pub fn resume(&mut self) -> Result<(), TpuError> {
        if self.state != TpuState::Suspended {
            return Err(TpuError::NotInitialized);
        }

        super::platform::restore_registers(&self.tdma, &self.tiu, &self.runtime.reg_backup);
        self.state = TpuState::Idle;
        
        Ok(())
    }

    /// 重置 TPU
    /// 
    /// 注意：实际的硬件复位需要操作系统提供复位控制器支持
    /// 这里只是重置软件状态和命令 ID
    pub fn reset(&mut self) {
        super::platform::resync_cmd_id(&self.tdma, &self.tiu);
        self.runtime = TpuRuntimeState::default();
        self.state = TpuState::Idle;
    }

    /// 获取 TDMA 寄存器引用 (用于高级操作)
    pub fn tdma(&self) -> &TdmaRegs {
        &self.tdma
    }

    /// 获取 TIU 寄存器引用 (用于高级操作)
    pub fn tiu(&self) -> &TiuRegs {
        &self.tiu
    }

    /// 获取运行时状态
    pub fn runtime_state(&self) -> &TpuRuntimeState {
        &self.runtime
    }
}

// 实现 Send 和 Sync (如果需要在多核环境使用，需要外部加锁)
unsafe impl Send for TpuDevice {}
unsafe impl Sync for TpuDevice {}

impl DeviceOps for TpuDevice {
    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> axfs_ng_vfs::VfsResult<usize> {
        Ok(0)
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> axfs_ng_vfs::VfsResult<usize> {
        Ok(0)
    }

    fn ioctl(&self, _cmd: u32, _arg: usize) -> axfs_ng_vfs::VfsResult<usize> {
        Ok(0)
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

}