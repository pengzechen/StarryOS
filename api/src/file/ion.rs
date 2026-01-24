//! Ion Buffer 文件类型
//!
//! 实现 FileLike trait，用于支持对 Ion 分配的缓冲区进行 mmap。

use alloc::borrow::Cow;

use axerrno::AxResult;
use axpoll::{IoEvents, Pollable};
use memory_addr::PhysAddrRange;

use super::{FileLike, Kstat};

/// Ion Buffer 的物理地址信息
#[derive(Debug, Clone)]
pub struct IonBufferInfo {
    /// 物理地址
    pub phys_addr: usize,
    /// 缓冲区大小
    pub size: usize,
    /// 缓冲区 handle
    pub handle: u32,
}

/// Ion Buffer 文件
///
/// 用于支持对 Ion 分配的缓冲区进行 mmap
pub struct IonBufferFile {
    /// 缓冲区信息
    info: IonBufferInfo,
}

impl IonBufferFile {
    /// 创建新的 Ion Buffer 文件
    pub fn new(info: IonBufferInfo) -> Self {
        Self { info }
    }

    /// 获取物理地址范围
    pub fn phys_range(&self) -> PhysAddrRange {
        PhysAddrRange::from_start_size(
            memory_addr::PhysAddr::from(self.info.phys_addr),
            self.info.size,
        )
    }

    /// 获取缓冲区信息
    pub fn info(&self) -> &IonBufferInfo {
        &self.info
    }
}

impl Pollable for IonBufferFile {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut core::task::Context<'_>, _events: IoEvents) {
        // Ion buffer 总是就绪
    }
}

impl FileLike for IonBufferFile {
    fn read(&self, _dst: &mut super::IoDst) -> AxResult<usize> {
        // Ion buffer 不支持直接读取
        Err(axerrno::AxError::InvalidInput)
    }

    fn write(&self, _src: &mut super::IoSrc) -> AxResult<usize> {
        // Ion buffer 不支持直接写入
        Err(axerrno::AxError::InvalidInput)
    }

    fn stat(&self) -> AxResult<Kstat> {
        Ok(Kstat {
            size: self.info.size as u64,
            ..Default::default()
        })
    }

    fn path(&self) -> Cow<'_, str> {
        Cow::Borrowed("/dev/ion_buffer")
    }
}
