//! RISC-V specific page table structures.

use memory_addr::VirtAddr;
use page_table_entry::riscv::Rv64PTE;

use crate::{PageTable64, PageTable64Mut, PagingMetaData};

#[inline]
fn riscv_flush_tlb(vaddr: Option<memory_addr::VirtAddr>) {
    if let Some(vaddr) = vaddr {
        riscv::asm::sfence_vma(0, vaddr.as_usize())
    } else {
        riscv::asm::sfence_vma_all();
    }
}

/// Metadata of RISC-V Sv39 page tables.
pub struct Sv39MetaData;

impl PagingMetaData for Sv39MetaData {
    const LEVELS: usize = 3;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 39;
    type VirtAddr = VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<Self::VirtAddr>) {
        riscv_flush_tlb(vaddr);
    }
}

/// Metadata of RISC-V Sv48 page tables.
pub struct Sv48MetaData;

impl PagingMetaData for Sv48MetaData {
    const LEVELS: usize = 4;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 48;
    type VirtAddr = VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<Self::VirtAddr>) {
        riscv_flush_tlb(vaddr);
    }
}

/// Sv39: Page-Based 39-bit (3 levels) Virtual-Memory System.
pub type Sv39PageTable<H> = PageTable64<Sv39MetaData, Rv64PTE, H>;
pub type Sv39PageTableMut<'a, H> = PageTable64Mut<'a, Sv39MetaData, Rv64PTE, H>;

/// Sv48: Page-Based 48-bit (4 levels) Virtual-Memory System.
pub type Sv48PageTable<H> = PageTable64<Sv48MetaData, Rv64PTE, H>;
pub type Sv48PageTableMut<'a, H> = PageTable64Mut<'a, Sv48MetaData, Rv64PTE, H>;
