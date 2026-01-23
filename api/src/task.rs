use core::{ffi::c_long, sync::atomic::Ordering};

use axerrno::{AxError, AxResult};
use axhal::uspace::{ExceptionKind, ReturnReason, UserContext};
use axtask::{TaskInner, current};
use bytemuck::AnyBitPattern;
use linux_raw_sys::general::ROBUST_LIST_LIMIT;
use memory_addr::VirtAddr;
use starry_core::{
    futex::FutexKey,
    shm::SHM_MANAGER,
    task::{
        AsThread, get_process_data, get_task, send_signal_to_process, send_signal_to_thread,
        set_timer_state,
    },
    time::TimerState,
};
use starry_process::Pid;
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use crate::{
    signal::{check_signals, unblock_next_signal},
    syscall::handle_syscall,
};

pub fn hexdump(data: &[u8], mut start_addr: usize) {
    const PRELAND_WIDTH: usize = 70;
    ax_println!("{:-^1$}", " hexdump ", PRELAND_WIDTH);
    for offset in (0..data.len()).step_by(16) {
        ax_print!("{:08x} ", start_addr);
        start_addr += 0x10;
        for i in 0..16 {
            if offset + i < data.len() {
                ax_print!("{:02x} ", data[offset + i]);
            } else {
                ax_print!("{:02} ", "");
            }
        }

        ax_print!("{:>6}", ' ');

        for i in 0..16 {
            if offset + i < data.len() {
                let c = data[offset + i];
                if c >= 0x20 && c <= 0x7e {
                    ax_print!("{}", c as char);
                } else {
                    ax_print!(".");
                }
            } else {
                ax_print!("{:02} ", "");
            }
        }

        ax_println!("");
    }
    ax_println!("{:-^1$}", " hexdump end ", PRELAND_WIDTH);
}




/// Create a new user task.
pub fn new_user_task(name: &str, mut uctx: UserContext, set_child_tid: usize) -> TaskInner {
    TaskInner::new(
        move || {
            let curr = axtask::current();

            if let Some(tid) = (set_child_tid as *mut Pid).nullable() {
                tid.vm_write(curr.id().as_u64() as Pid).ok();
            }

            info!("Enter user space: ip={:#x}, sp={:#x}", uctx.ip(), uctx.sp());

            let thr = curr.as_thread();
            while !thr.pending_exit() {
                unsafe {
                    // Ensure instruction fetch sees latest memory contents
                    core::arch::asm!(".long 0x0100000b"); // 15.1.13 ICACHE.IALL
                    core::arch::asm!(".long 0x01a0000b"); // 15.2.2 SYNC.I
                }
                let reason = uctx.run();

                set_timer_state(&curr, TimerState::Kernel);

                match reason {
                    ReturnReason::Syscall => handle_syscall(&mut uctx),
                    ReturnReason::PageFault(addr, flags) => {
                        if !thr.proc_data.aspace.lock().handle_page_fault(addr, flags) {
                            info!(
                                "{:?}: segmentation fault at {:#x} {:?} pc:{:#x}",
                                thr.proc_data.proc,
                                addr,
                                flags,
                                uctx.ip()
                            );
                            raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                                .expect("Failed to send SIGSEGV");
                        }
                    }
                    ReturnReason::Interrupt => {}
                    #[allow(unused_labels)]
                    ReturnReason::Exception(exc_info) => 'exc: {
                        // TODO: detailed handling
                        let signo = match exc_info.kind() {
                            ExceptionKind::Misaligned => {
                                #[cfg(target_arch = "loongarch64")]
                                if unsafe { uctx.emulate_unaligned() }.is_ok() {
                                    break 'exc;
                                }
                                Signo::SIGBUS
                            }
                            ExceptionKind::Breakpoint => Signo::SIGTRAP,
                            ExceptionKind::IllegalInstruction => Signo::SIGILL,
                            _ => Signo::SIGTRAP,
                        };
                        warn!(
                            "{:?}: exception {:?} at ip={:#x}",
                            thr.proc_data.proc,
                            exc_info.kind(),
                            uctx.ip()
                        );
                        warn!("stval = {:#?}", exc_info);
                        let mut buf = [0u8; 1024];
                        let _ = thr
                            .proc_data
                            .aspace
                            .lock()
                            .read(VirtAddr::from_usize(uctx.ip()), &mut buf);
                        info!("code bytes: {:02x?}", &buf[..16]);

                        hexdump(&buf[..1024], uctx.ip());
                        let value: usize;
                        unsafe {
                            core::arch::asm!(
                                "csrr {0}, sstatus",
                                out(reg) value,
                                options(nomem, nostack, preserves_flags),
                            );
                        }
                        info!("sstatus = {:#018x}", value);
                        let scause: usize;
                        unsafe {
                            core::arch::asm!(
                                "csrr {0}, scause",
                                out(reg) scause,
                                options(nomem, nostack, preserves_flags),
                            );
                        }
                        info!("scause = {:#018x}", scause);
                        // 1000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0101
                        raise_signal_fatal(SignalInfo::new_kernel(signo))
                            .expect("Failed to send SIGTRAP");
                    }
                    r => {
                        warn!("Unexpected return reason: {r:?}");
                        raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                            .expect("Failed to send SIGSEGV");
                    }
                }

                if !unblock_next_signal() {
                    while check_signals(thr, &mut uctx, None) {}
                }

                set_timer_state(&curr, TimerState::User);
                curr.clear_interrupt();
            }
        },
        name.into(),
        starry_core::config::KERNEL_STACK_SIZE,
    )
}

#[repr(C)]
#[derive(Debug, Copy, Clone, AnyBitPattern)]
pub struct RobustList {
    pub next: *mut RobustList,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, AnyBitPattern)]
pub struct RobustListHead {
    pub list: RobustList,
    pub futex_offset: c_long,
    pub list_op_pending: *mut RobustList,
}

fn handle_futex_death(entry: *mut RobustList, offset: i64) -> AxResult<()> {
    let address = (entry as u64)
        .checked_add_signed(offset)
        .ok_or(AxError::InvalidInput)?;
    let address: usize = address.try_into().map_err(|_| AxError::InvalidInput)?;
    let key = FutexKey::new_current(address);

    let curr = current();
    let futex_table = curr.as_thread().proc_data.futex_table_for(&key);

    let Some(futex) = futex_table.get(&key) else {
        return Ok(());
    };
    futex.owner_dead.store(true, Ordering::SeqCst);
    futex.wq.wake(1, u32::MAX);
    Ok(())
}

pub fn exit_robust_list(head: *const RobustListHead) -> AxResult<()> {
    // Reference: https://elixir.bootlin.com/linux/v6.13.6/source/kernel/futex/core.c#L777

    let mut limit = ROBUST_LIST_LIMIT;

    let end_ptr = unsafe { &raw const (*head).list };
    let head = head.vm_read()?;
    let mut entry = head.list.next;
    let offset = head.futex_offset;
    let pending = head.list_op_pending;

    while !core::ptr::eq(entry, end_ptr) {
        let next_entry = entry.vm_read()?.next;
        if entry != pending {
            handle_futex_death(entry, offset)?;
        }
        entry = next_entry;

        limit -= 1;
        if limit == 0 {
            return Err(AxError::FilesystemLoop);
        }
        axtask::yield_now();
    }

    Ok(())
}

pub fn do_exit(exit_code: i32, group_exit: bool) {
    let curr = current();
    let thr = curr.as_thread();

    info!("{} exit with code: {}", curr.id_name(), exit_code);

    let clear_child_tid = thr.clear_child_tid() as *mut u32;
    if clear_child_tid.vm_write(0).is_ok() {
        let key = FutexKey::new_current(clear_child_tid as usize);
        let table = thr.proc_data.futex_table_for(&key);
        let guard = table.get(&key);
        if let Some(futex) = guard {
            futex.wq.wake(1, u32::MAX);
        }
        axtask::yield_now();
    }
    let head = thr.robust_list_head() as *const RobustListHead;
    if !head.is_null()
        && let Err(err) = exit_robust_list(head)
    {
        warn!("exit robust list failed: {err:?}");
    }

    let process = &thr.proc_data.proc;
    if process.exit_thread(curr.id().as_u64() as Pid, exit_code) {
        process.exit();
        if let Some(parent) = process.parent() {
            if let Some(signo) = thr.proc_data.exit_signal {
                let _ = send_signal_to_process(parent.pid(), Some(SignalInfo::new_kernel(signo)));
            }
            if let Ok(data) = get_process_data(parent.pid()) {
                data.child_exit_event.wake();
            }
        }
        thr.proc_data.exit_event.wake();

        SHM_MANAGER.lock().clear_proc_shm(process.pid());
    }
    if group_exit && !process.is_group_exited() {
        process.group_exit();
        let sig = SignalInfo::new_kernel(Signo::SIGKILL);
        for tid in process.threads() {
            let _ = send_signal_to_thread(None, tid, Some(sig.clone()));
        }
    }
    thr.set_exit();
}

/// Sends a fatal signal to the current process.
pub fn raise_signal_fatal(sig: SignalInfo) -> AxResult<()> {
    let curr = current();
    let proc_data = &curr.as_thread().proc_data;

    let signo = sig.signo();
    info!("Send fatal signal {signo:?} to the current process");
    if let Some(tid) = proc_data.signal.send_signal(sig)
        && let Ok(task) = get_task(tid)
    {
        task.interrupt();
    } else {
        // No task wants to handle the signal, abort the task
        do_exit(signo as i32, true);
    }

    Ok(())
}
