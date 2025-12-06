use crate::{GenericPTE, PagingHandler, PagingMetaData};
use crate::{MappingFlags, PageSize, PagingError, PagingResult, TlbFlush, TlbFlushAll};
use core::marker::PhantomData;
use memory_addr::{MemoryAddr, PAGE_SIZE_4K, PhysAddr};

const ENTRY_COUNT_L1: usize = 4096; // ARMv7-A L1 has 4096 entries
const ENTRY_COUNT_L2: usize = 256;  // ARMv7-A L2 has 256 entries

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 20) & 0xFFF // bits[31:20] for 1MB sections
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> 12) & 0xFF // bits[19:12] for 4KB pages
}

/// A generic page table struct for 32-bit ARM platform (ARMv7-A).
///
/// This implements a 2-level page table:
/// - L1: 4096 entries, each covering 1MB (Section) or pointing to L2
/// - L2: 256 entries, each covering 4KB (Small Page)
///
/// It tracks all L2 tables for proper deallocation.
pub struct PageTable32<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> {
    root_paddr: PhysAddr,
    _phantom: PhantomData<(M, PTE, H)>,
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable32<M, PTE, H> {
    /// Creates a new page table instance or returns the error.
    ///
    /// It will allocate a new 16KB aligned page for the L1 page table.
    pub fn try_new() -> PagingResult<Self> {
        let root_paddr = Self::alloc_table()?;
        Ok(Self {
            root_paddr,
            _phantom: PhantomData,
        })
    }

    /// Returns the physical address of the root page table (L1).
    pub const fn root_paddr(&self) -> PhysAddr {
        self.root_paddr
    }

    /// Maps a virtual page to a physical frame with the given `page_size`
    /// and mapping `flags`.
    ///
    /// - For 1MB sections: maps directly in L1
    /// - For 4KB pages: creates L2 table if needed, then maps in L2
    ///
    /// Returns [`Err(PagingError::AlreadyMapped)`](PagingError::AlreadyMapped)
    /// if the mapping is already present.
    pub fn map(
        &mut self,
        vaddr: M::VirtAddr,
        target: PhysAddr,
        page_size: PageSize,
        flags: MappingFlags,
    ) -> PagingResult<TlbFlush<M>> {
        let entry = self.get_entry_mut_or_create(vaddr, page_size)?;
        if !entry.is_unused() {
            return Err(PagingError::AlreadyMapped);
        }
        *entry = GenericPTE::new_page(target.align_down(page_size), flags, page_size.is_huge());
        Ok(TlbFlush::new(vaddr))
    }

    /// Unmaps the mapping starts with `vaddr`.
    ///
    /// Returns the page size of the unmapped mapping.
    pub fn unmap(&mut self, vaddr: M::VirtAddr) -> PagingResult<(PhysAddr, PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        let paddr = entry.paddr();
        entry.clear();
        Ok((paddr, size, TlbFlush::new(vaddr)))
    }

    /// Query the result of the mapping starts with `vaddr`.
    ///
    /// Returns the physical address of the target frame, the page size, and the
    /// flags of the mapping.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the mapping
    /// is not present.
    pub fn query(&self, vaddr: M::VirtAddr) -> PagingResult<(PhysAddr, PageSize, MappingFlags)> {
        let (entry, size) = self.get_entry(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        let off = vaddr.as_usize() & (size - 1);
        Ok((entry.paddr().add(off), size, entry.flags()))
    }

    fn alloc_table() -> PagingResult<PhysAddr> {
        H::alloc_frame().ok_or(PagingError::NoMemory)
    }

    fn get_entry_mut(&mut self, vaddr: M::VirtAddr) -> PagingResult<(&mut PTE, PageSize)> {
        let vaddr_usize = vaddr.as_usize();
        let p1 = p1_index(vaddr_usize);
        let table = self.get_table_mut(self.root_paddr);
        let entry = &mut table[p1];

        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }

        // Check if it's a 1MB Section
        if entry.is_huge() {
            return Ok((entry, PageSize::Size1M));
        }

        // It's a page table pointer, go to L2
        let p2_table = self.get_table_mut(entry.paddr());
        let p2 = p2_index(vaddr_usize);
        Ok((&mut p2_table[p2], PageSize::Size4K))
    }

    fn get_entry(&self, vaddr: M::VirtAddr) -> PagingResult<(&PTE, PageSize)> {
        let vaddr_usize = vaddr.as_usize();
        let p1 = p1_index(vaddr_usize);
        let table = self.get_table(self.root_paddr);
        let entry = &table[p1];

        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }

        if entry.is_huge() {
            return Ok((entry, PageSize::Size1M));
        }

        let p2_table = self.get_table(entry.paddr());
        let p2 = p2_index(vaddr_usize);
        Ok((&p2_table[p2], PageSize::Size4K))
    }

    fn get_entry_mut_or_create(
        &mut self,
        vaddr: M::VirtAddr,
        page_size: PageSize,
    ) -> PagingResult<&mut PTE> {
        let vaddr_usize = vaddr.as_usize();
        let p1 = p1_index(vaddr_usize);
        let table = self.get_table_mut(self.root_paddr);

        if page_size == PageSize::Size1M {
            // Map as 1MB Section in L1
            return Ok(&mut table[p1]);
        }

        // Need L2 page table for 4KB mapping
        let entry = &mut table[p1];
        if entry.is_unused() {
            // Create new L2 page table
            let paddr = Self::alloc_table()?;
            *entry = GenericPTE::new_table(paddr);
        } else if entry.is_huge() {
            // Already mapped as huge page
            return Err(PagingError::AlreadyMapped);
        }

        let p2_table = self.get_table_mut(entry.paddr());
        let p2 = p2_index(vaddr_usize);
        Ok(&mut p2_table[p2])
    }

    fn get_table(&self, paddr: PhysAddr) -> &[PTE] {
        let ptr = H::phys_to_virt(paddr).as_ptr() as *const PTE;
        unsafe { core::slice::from_raw_parts(ptr, ENTRY_COUNT_L1) }
    }

    fn get_table_mut(&self, paddr: PhysAddr) -> &mut [PTE] {
        let ptr = H::phys_to_virt(paddr).as_mut_ptr() as *mut PTE;
        unsafe { core::slice::from_raw_parts_mut(ptr, ENTRY_COUNT_L1) }
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable32<M, PTE, H> {
    /// Creates a new page table instance or panics if allocation fails.
    pub fn new() -> Self {
        Self::try_new().expect("Failed to allocate root page table")
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> Drop for PageTable32<M, PTE, H> {
    fn drop(&mut self) {
        // Deallocate all L2 page tables
        let table = self.get_table(self.root_paddr);
        for entry in table {
            if !entry.is_unused() && !entry.is_huge() {
                // This is an L2 page table
                H::dealloc_frame(entry.paddr());
            }
        }
        // Deallocate L1 page table
        H::dealloc_frame(self.root_paddr);
    }
}
