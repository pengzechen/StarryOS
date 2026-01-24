//! Ion 设备实现

use super::buffer::IonBufferManager;
use super::error::{IonError, IonResult};
use super::heap::IonHeapManager;
use super::types::*;
use super::types::ioctl::*;
use crate::vfs::DeviceOps;
use alloc::sync::Arc;
use axfs_ng_vfs::{NodeFlags, VfsResult};
use core::any::Any;
use core::ptr;

/// Ion 设备
pub struct IonDevice {
    /// 堆管理器
    heap_manager: IonHeapManager,
    /// 缓冲区管理器
    buffer_manager: IonBufferManager,
}

impl IonDevice {
    /// 创建 Ion 设备
    pub const fn new() -> Self {
        Self {
            heap_manager: IonHeapManager::new(),
            buffer_manager: IonBufferManager::new(),
        }
    }
    
    /// 初始化设备
    pub fn init(&self) -> IonResult<()> {
        info!("Initializing Ion device");
        info!("Supported heap mask: 0x{:x}", self.heap_manager.supported_heap_mask());
        Ok(())
    }
    
    /// 处理 ION_IOC_ALLOC 命令
    fn handle_alloc(&self, user_ptr: usize) -> VfsResult<usize> {
        debug!("Processing ION_IOC_ALLOC");
        
        // 从用户空间读取分配数据
        let alloc_data = unsafe {
            ptr::read(user_ptr as *const IonAllocData)
        };
        
        debug!(
            "Alloc request: len={}, align={}, heap_id_mask=0x{:x}, flags=0x{:x}",
            alloc_data.len, alloc_data.align, alloc_data.heap_id_mask, alloc_data.flags
        );
        
        // 选择堆类型（简化处理，优先选择 DMA coherent）
        let heap_type = if (alloc_data.heap_id_mask & (1 << IonHeapType::DmaCoherent as u32)) != 0 {
            IonHeapType::DmaCoherent
        } else if (alloc_data.heap_id_mask & (1 << IonHeapType::System as u32)) != 0 {
            IonHeapType::System
        } else {
            error!("No supported heap type in mask: 0x{:x}", alloc_data.heap_id_mask);
            return Err(axerrno::AxError::InvalidInput);
        };
        
        // 分配缓冲区
        let buffer = self.heap_manager.alloc_buffer(
            alloc_data.len as usize,
            alloc_data.align.max(1) as usize,
            heap_type,
            IonFlags(alloc_data.flags),
        ).map_err(|e| axerrno::AxError::from(e))?;
        
        // 注册缓冲区
        self.buffer_manager.register_buffer(buffer.clone())
            .map_err(|e| axerrno::AxError::from(e))?;
        
        // 返回结果（简化处理，直接返回 handle 作为 fd）
        let mut result_data = alloc_data;
        result_data.fd = buffer.handle.as_u32() as i32;
        
        unsafe {
            ptr::write(user_ptr as *mut IonAllocData, result_data);
        }
        
        info!("Allocated Ion buffer: handle={}, size={}", 
              result_data.fd, alloc_data.len);
        
        Ok(0)
    }
    
    /// 处理 ION_IOC_FREE 命令
    fn handle_free(&self, user_ptr: usize) -> VfsResult<usize> {
        debug!("Processing ION_IOC_FREE");
        
        // 从用户空间读取句柄数据
        let handle_data = unsafe {
            ptr::read(user_ptr as *const IonHandleData)
        };
        
        let handle = IonHandle(handle_data.handle);
        debug!("Freeing buffer with handle: {:?}", handle);
        
        // 取消注册缓冲区
        let buffer = self.buffer_manager.unregister_buffer(handle)
            .map_err(|e| axerrno::AxError::from(e))?;
        
        // 释放缓冲区
        self.heap_manager.free_buffer(buffer)
            .map_err(|e| axerrno::AxError::from(e))?;
        
        info!("Freed Ion buffer: handle={}", handle_data.handle);
        Ok(0)
    }
    
    /// 处理 ION_IOC_IMPORT 命令
    fn handle_import(&self, user_ptr: usize) -> VfsResult<usize> {
        debug!("Processing ION_IOC_IMPORT");
        
        // 从用户空间读取 FD 数据
        let fd_data = unsafe {
            ptr::read(user_ptr as *const IonFdData)
        };
        
        debug!("Import request: fd={}", fd_data.fd);
        
        // 简化处理：将 fd 作为 handle 直接使用
        // 实际实现中应该检查 fd 的有效性
        let handle = IonHandle(fd_data.fd as u32);
        
        // 返回结果
        let mut result_data = fd_data;
        result_data.handle = handle.as_u32();
        
        unsafe {
            ptr::write(user_ptr as *mut IonFdData, result_data);
        }
        
        info!("Imported Ion buffer: fd={}, handle={}", fd_data.fd, result_data.handle);
        Ok(0)
    }
    
    /// 处理 ION_IOC_HEAP_QUERY 命令
    fn handle_heap_query(&self, user_ptr: usize) -> VfsResult<usize> {
        debug!("Processing ION_IOC_HEAP_QUERY");
        
        // 从用户空间读取查询数据
        let mut heap_query = unsafe {
            ptr::read(user_ptr as *const IonHeapQuery)
        };
        
        debug!("Heap query request: cnt={}", heap_query.cnt);
        
        // 获取支持的堆类型信息
        let supported_heaps = [
            (IonHeapType::System, "system"),
            (IonHeapType::DmaCoherent, "dma_coherent"),
            (IonHeapType::Carveout, "carveout"),
        ];
        
        let available_heap_count = supported_heaps.len() as u32;
        
        // 如果用户提供的缓冲区大小足够，我们可以填充堆信息
        // 这里简化处理，只返回堆数量
        heap_query.cnt = available_heap_count;
        
        // 写回结果
        unsafe {
            ptr::write(user_ptr as *mut IonHeapQuery, heap_query);
        }
        
        info!("Heap query completed: {} heaps available", available_heap_count);
        Ok(0)
    }
}

impl DeviceOps for IonDevice {
    fn read_at(&self, _buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        // Ion 设备不支持直接读写
        Ok(0)
    }
    
    fn write_at(&self, _buf: &[u8], _offset: u64) -> VfsResult<usize> {
        // Ion 设备不支持直接读写
        Ok(0)
    }
    
    fn ioctl(&self, cmd: u32, arg: usize) -> VfsResult<usize> {
        match cmd {
            ION_IOC_ALLOC => self.handle_alloc(arg),
            ION_IOC_FREE => self.handle_free(arg),
            ION_IOC_IMPORT => self.handle_import(arg),
            ION_IOC_HEAP_QUERY => self.handle_heap_query(arg),
            _ => {
                warn!("Unsupported Ion ioctl command: 0x{:x}", cmd);
                Err(axerrno::AxError::Unsupported)
            }
        }
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE
    }
}

impl Drop for IonDevice {
    fn drop(&mut self) {
        warn!("Ion device is being dropped, cleaning up buffers");
        self.buffer_manager.cleanup_all();
    }
}
