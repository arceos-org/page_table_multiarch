//! x86 specific page table structures.

use crate::{PageTable64, PagingMetaData};
use page_table_entry::x86_64::X64PTE;

/// metadata of x86_64 page tables.
pub struct X64PagingMetaData;

impl PagingMetaData for X64PagingMetaData {
    const LEVELS: usize = 4;
    const PA_MAX_BITS: usize = 52;
    const VA_MAX_BITS: usize = 48;
    type VirtAddr = memory_addr::VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<memory_addr::VirtAddr>) {
        #[cfg(target_arch = "x86_64")]
        if let Some(vaddr) = vaddr {
            x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(vaddr.as_usize() as u64));
        } else {
            x86_64::instructions::tlb::flush_all();
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = vaddr;
    }
}

/// x86_64 page table.
pub type X64PageTable<H> = PageTable64<X64PagingMetaData, X64PTE, H>;
