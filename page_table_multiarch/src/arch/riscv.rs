//! RISC-V specific page table structures.

use crate::{PageTable64, PagingMetaData};
use page_table_entry::riscv::Rv64PTE;

#[inline]
fn riscv_flush_tlb(vaddr: Option<memory_addr::VirtAddr>) {
    unsafe {
        if let Some(vaddr) = vaddr {
            riscv::asm::sfence_vma(0, vaddr.as_usize())
        } else {
            riscv::asm::sfence_vma_all();
        }
    }
}

/// Page table metadata for RISC-V Sv-39 and Sv-48 page tables.
///
/// This trait is used to allow them to support both normal page tables and
/// nested page tables.
pub trait SvMetaData: Sync + Send {
    type VirtAddr: memory_addr::MemoryAddr;

    fn flush_tlb(vaddr: Option<Self::VirtAddr>);
}

/// Metadata of RISC-V Sv39 page tables.
pub struct Sv39MetaData<M: SvMetaData> {
    _virt_addr: core::marker::PhantomData<M>,
}

/// Metadata of RISC-V Sv48 page tables.
pub struct Sv48MetaData<M: SvMetaData> {
    _virt_addr: core::marker::PhantomData<M>,
}

impl<M: SvMetaData> PagingMetaData for Sv39MetaData<M> {
    const LEVELS: usize = 3;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 39;
    type VirtAddr = M::VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<M::VirtAddr>) {
        M::flush_tlb(vaddr);
    }
}

impl<M: SvMetaData> PagingMetaData for Sv48MetaData<M> {
    const LEVELS: usize = 4;
    const PA_MAX_BITS: usize = 56;
    const VA_MAX_BITS: usize = 48;
    type VirtAddr = M::VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<M::VirtAddr>) {
        M::flush_tlb(vaddr);
    }
}

/// Metadata for normal (`VirtAddr` to `PhysAddr`) Sv39/Sv48 page tables.
pub struct NormalSvPageTable;

impl SvMetaData for NormalSvPageTable {
    type VirtAddr = memory_addr::VirtAddr;

    #[inline]
    fn flush_tlb(vaddr: Option<memory_addr::VirtAddr>) {
        riscv_flush_tlb(vaddr)
    }
}

/// Sv39 page table for some virtual address type and flush function.
pub type Sv39PageTableGeneric<M, H> = PageTable64<Sv39MetaData<M>, Rv64PTE, H>;

/// Sv48 page table for some virtual address type and flush function.
pub type Sv48PageTableGeneric<M, H> = PageTable64<Sv48MetaData<M>, Rv64PTE, H>;

/// Sv39: Page-Based 39-bit (3 levels) Virtual-Memory System.
pub type Sv39PageTable<H> = Sv39PageTableGeneric<NormalSvPageTable, H>;

/// Sv48: Page-Based 48-bit (4 levels) Virtual-Memory System.
pub type Sv48PageTable<H> = Sv48PageTableGeneric<NormalSvPageTable, H>;
