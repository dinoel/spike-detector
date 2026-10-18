//! Snapshot of every process on the machine from a single
//! `NtQuerySystemInformation(SystemProcessInformation)` call: the data Task
//! Manager shows, available without elevation.

use std::{io, mem, ptr};

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::Foundation::{STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING};

use crate::win::{as_bytes, utf16_in};

/// Identity of a process instance. PIDs get recycled; (PID, creation time)
/// does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: u32,
    /// FILETIME units: 100 ns intervals since 1601-01-01 UTC.
    pub create_time: i64,
}

/// One process as reported by a snapshot. Borrows its name from the
/// snapshot buffer.
#[derive(Debug, Clone, Copy)]
pub struct RawProcess<'a> {
    pub key: ProcessKey,
    pub parent_pid: u32,
    /// Terminal Services session; 0 is where services run.
    pub session_id: u32,
    /// User + kernel time, 100 ns units.
    pub cpu_time: u64,
    pub private_bytes: u64,
    pub handle_count: u32,
    pub thread_count: u32,
    /// Bytes read and written through the process's own handles since it
    /// started: files (cache hits included) and pipes. Most socket traffic
    /// goes through device-control calls and isn't counted here.
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    /// Image name such as `node.exe`, UTF-16. Empty for the idle process.
    pub name: &'a [u16],
}

pub struct Sampler {
    size_hint: usize,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            size_hint: 512 * 1024,
        }
    }

    /// Takes a fresh snapshot.
    ///
    /// The buffer is several hundred KB, mostly per-thread records we don't
    /// need. It lives only as long as the returned [`Snapshot`], so between
    /// ticks the app holds none of it.
    pub fn take(&mut self) -> io::Result<Snapshot> {
        const MAX_ATTEMPTS: usize = 8;
        for _ in 0..MAX_ATTEMPTS {
            let mut buf = vec![0u64; self.size_hint.div_ceil(8)];
            let capacity = u32::try_from(buf.len() * 8).unwrap_or(u32::MAX);
            let mut needed = 0u32;
            // SAFETY: the pointer and length describe `buf`, which outlives the call.
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    buf.as_mut_ptr().cast(),
                    capacity,
                    &mut needed,
                )
            };
            let needed = needed as usize;
            if status >= 0 {
                // Track the real size (up or down) with headroom, so the next
                // call usually fits on the first try.
                self.size_hint = needed + needed / 4;
                return Ok(Snapshot { buf });
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return Err(io::Error::other(format!(
                    "NtQuerySystemInformation: NTSTATUS {status:#010x}"
                )));
            }
            // Processes may start between calls, so ask for more than reported.
            self.size_hint = (needed + needed / 4).max(self.size_hint * 2);
        }
        Err(io::Error::other(
            "NtQuerySystemInformation: process list keeps outgrowing the buffer",
        ))
    }
}

pub struct Snapshot {
    buf: Vec<u64>,
}

impl Snapshot {
    pub fn processes(&self) -> Processes<'_> {
        Processes {
            bytes: as_bytes(&self.buf),
            next: Some(0),
        }
    }
}

pub struct Processes<'a> {
    bytes: &'a [u8],
    next: Option<usize>,
}

impl<'a> Iterator for Processes<'a> {
    type Item = RawProcess<'a>;

    fn next(&mut self) -> Option<RawProcess<'a>> {
        let offset = self.next.take()?;
        let end = offset.checked_add(mem::size_of::<SystemProcessInformation>())?;
        let record = self.bytes.get(offset..end)?;
        // SAFETY: `record` spans a whole struct, `read_unaligned` has no
        // alignment requirement, and every field is plain data.
        let info: SystemProcessInformation = unsafe { ptr::read_unaligned(record.as_ptr().cast()) };
        if info.next_entry_offset != 0 {
            self.next = offset.checked_add(info.next_entry_offset as usize);
        }
        Some(RawProcess {
            key: ProcessKey {
                // PIDs are DWORDs stored in a HANDLE-sized field.
                pid: info.unique_process_id as u32,
                create_time: info.create_time,
            },
            parent_pid: info.inherited_from_unique_process_id as u32,
            session_id: info.session_id,
            cpu_time: u64::try_from(info.user_time + info.kernel_time).unwrap_or(0),
            // Despite its name, this field is in bytes.
            private_bytes: info.private_page_count as u64,
            handle_count: info.handle_count,
            io_read_bytes: u64::try_from(info.read_transfer_count).unwrap_or(0),
            io_write_bytes: u64::try_from(info.write_transfer_count).unwrap_or(0),
            thread_count: info.number_of_threads,
            name: utf16_in(self.bytes, &info.image_name).unwrap_or_default(),
        })
    }
}

/// `SYSTEM_PROCESS_INFORMATION` as documented by the phnt headers; the SDK's
/// winternl.h hides most of these fields behind `Reserved*` arrays.
#[repr(C)]
#[allow(
    dead_code,
    reason = "mirrors the OS layout; unused fields keep the offsets right"
)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UNICODE_STRING,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
    // Followed by `number_of_threads` SYSTEM_THREAD_INFORMATION records.
}

// Known x64 offsets: a mistake in the struct above fails the build instead
// of silently reading garbage.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(mem::offset_of!(SystemProcessInformation, create_time) == 0x20);
    assert!(mem::offset_of!(SystemProcessInformation, image_name) == 0x38);
    assert!(mem::offset_of!(SystemProcessInformation, unique_process_id) == 0x50);
    assert!(mem::offset_of!(SystemProcessInformation, private_page_count) == 0xC8);
    assert!(mem::size_of::<SystemProcessInformation>() == 0x100);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_includes_this_process() {
        let snapshot = Sampler::new().take().unwrap();
        let me = snapshot
            .processes()
            .find(|p| p.key.pid == std::process::id())
            .expect("own process missing from snapshot");
        let name = String::from_utf16(me.name).unwrap();
        assert!(name.to_lowercase().ends_with(".exe"), "{name}");
        assert!(me.key.create_time > 0);
        assert!(me.private_bytes > 0);
        assert!(snapshot.processes().count() > 10);
    }

    #[test]
    fn grows_buffer_when_hint_is_too_small() {
        let mut sampler = Sampler { size_hint: 64 };
        let snapshot = sampler.take().unwrap();
        assert!(snapshot.processes().count() > 10);
        assert!(sampler.size_hint > 64);
    }
}
