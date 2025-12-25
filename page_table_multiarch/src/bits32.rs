use crate::{GenericPTE, PagingHandler, PagingMetaData};
use crate::{MappingFlags, PageSize, PagingError, PagingResult, TlbFlush, TlbFlushAll};
use core::marker::PhantomData;
use memory_addr::{MemoryAddr, PAGE_SIZE_4K, PhysAddr};

#[cfg(target_arch = "arm")]
const ENTRY_COUNT: usize = 4096; // ARMv7-A L1 has 4096 entries
#[cfg(not(target_arch = "arm"))]
const ENTRY_COUNT: usize = 512; // 512 entries per table

/// Extract the L1 (first-level) page table index from a virtual address.
/// 
/// For ARMv7-A:
/// - L1 uses bits[31:20] of the virtual address (12 bits = 4096 entries)
/// - Each L1 entry covers 1MB of virtual address space
const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 20) & 0xFFF // bits[31:20] for 1MB sections
}

/// Extract the L2 (second-level) page table index from a virtual address.
/// 
/// For ARMv7-A:
/// - L2 uses bits[19:12] of the virtual address (8 bits = 256 entries)
/// - Each L2 entry covers 4KB of virtual address space
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
        let (root_paddr, size_pages) = {
            #[cfg(target_arch = "arm")]
            {
                // ARMv7-A L1 page table: 4096 entries * 4 bytes = 16KB
                // Requires 16KB alignment for TTBR0
                const L1_SIZE_PAGES: usize = 4; // 16KB = 4 * 4KB
                const L1_ALIGN: usize = 16384; // 16KB alignment

                let root_paddr = H::alloc_frame_contiguous(L1_SIZE_PAGES, L1_ALIGN)
                    .ok_or(PagingError::NoMemory)?;

                (root_paddr, L1_SIZE_PAGES)
            }

            #[cfg(not(target_arch = "arm"))]
            {
                // Other 32-bit architectures page table: 512 entries * 8 bytes = 4KB
                const SIZE_PAGES: usize = 1; // 4KB = 1 * 4KB
                let root_paddr = H::alloc_frame().ok_or(PagingError::NoMemory)?;
                (root_paddr, SIZE_PAGES)
            }
        };

        // Zero out the root page table
        let virt = H::phys_to_virt(root_paddr);
        unsafe {
            core::ptr::write_bytes(virt.as_mut_ptr(), 0, size_pages * PAGE_SIZE_4K);
        }

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
        let off = vaddr.into() & (size as usize - 1);
        Ok((entry.paddr().add(off), size, entry.flags()))
    }

    fn get_entry_mut(&mut self, vaddr: M::VirtAddr) -> PagingResult<(&mut PTE, PageSize)> {
        let vaddr_usize = vaddr.into();
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
        let vaddr_usize = vaddr.into();
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
        let vaddr_usize = vaddr.into();
        let p1 = p1_index(vaddr_usize);
        let table = self.get_table_mut(self.root_paddr);

        if page_size == PageSize::Size1M {
            // Map as 1MB Section in L1
            return Ok(&mut table[p1]);
        }

        // Need L2 page table for 4KB mapping
        let entry = &mut table[p1];
        if entry.is_unused() {
            // Create new L2 page table (allocate 4KB, though only 1KB is used)
            let paddr = H::alloc_frame().ok_or(PagingError::NoMemory)?;

            // Zero out the L2 page table
            let virt = H::phys_to_virt(paddr);
            unsafe {
                core::ptr::write_bytes(virt.as_mut_ptr(), 0, PAGE_SIZE_4K);
            }

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
        unsafe { core::slice::from_raw_parts(ptr, ENTRY_COUNT) }
    }

    fn get_table_mut(&self, paddr: PhysAddr) -> &mut [PTE] {
        let ptr = H::phys_to_virt(paddr).as_mut_ptr() as *mut PTE;
        unsafe { core::slice::from_raw_parts_mut(ptr, ENTRY_COUNT) }
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> PageTable32<M, PTE, H> {
    /// Creates a new page table instance or panics if allocation fails.
    pub fn new() -> Self {
        Self::try_new().expect("Failed to allocate root page table")
    }

    /// Remap the mapping starts with `vaddr`, updates both the physical address
    /// and flags.
    ///
    /// Returns the page size of the mapping.
    pub fn remap(
        &mut self,
        vaddr: M::VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
    ) -> PagingResult<(PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        *entry = GenericPTE::new_page(paddr, flags, size.is_huge());
        Ok((size, TlbFlush::new(vaddr)))
    }

    /// Updates the flags of the mapping starts with `vaddr`.
    ///
    /// Returns the page size of the mapping.
    pub fn protect(
        &mut self,
        vaddr: M::VirtAddr,
        flags: MappingFlags,
    ) -> PagingResult<(PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        *entry = GenericPTE::new_page(entry.paddr(), flags, size.is_huge());
        Ok((size, TlbFlush::new(vaddr)))
    }

    /// Maps a contiguous virtual memory region to a contiguous physical memory
    /// region with the given mapping `flags`.
    pub fn map_region(
        &mut self,
        vaddr: M::VirtAddr,
        get_paddr: impl Fn(M::VirtAddr) -> PhysAddr,
        size: usize,
        flags: MappingFlags,
        allow_huge: bool,
        flush_tlb_by_page: bool,
    ) -> PagingResult<TlbFlushAll<M>> {
        let mut vaddr_usize: usize = vaddr.into();
        let mut size = size;
        if !PageSize::Size4K.is_aligned(vaddr_usize) || !PageSize::Size4K.is_aligned(size) {
            return Err(PagingError::NotAligned);
        }
        trace!(
            "map_region({:#x}): [{:#x}, {:#x}) {:?}",
            self.root_paddr(),
            vaddr_usize,
            vaddr_usize + size,
            flags,
        );
        while size > 0 {
            let vaddr = vaddr_usize.into();
            let paddr = get_paddr(vaddr);
            let page_size = if allow_huge
                && PageSize::Size1M.is_aligned(vaddr_usize)
                && paddr.is_aligned(PageSize::Size1M)
                && size >= PageSize::Size1M as usize
            {
                PageSize::Size1M
            } else {
                PageSize::Size4K
            };
            let tlb = self.map(vaddr, paddr, page_size, flags).inspect_err(|e| {
                error!("failed to map page: {vaddr_usize:#x?}({page_size:?}) -> {paddr:#x?}, {e:?}")
            })?;
            if flush_tlb_by_page {
                tlb.flush();
            } else {
                tlb.ignore();
            }

            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(TlbFlushAll::new())
    }

    /// Unmaps a contiguous virtual memory region.
    pub fn unmap_region(
        &mut self,
        vaddr: M::VirtAddr,
        size: usize,
        flush_tlb_by_page: bool,
    ) -> PagingResult<TlbFlushAll<M>> {
        let mut vaddr_usize: usize = vaddr.into();
        let mut size = size;
        trace!(
            "unmap_region({:#x}) [{:#x}, {:#x})",
            self.root_paddr(),
            vaddr_usize,
            vaddr_usize + size,
        );
        while size > 0 {
            let vaddr = vaddr_usize.into();
            let (_, page_size, tlb) = self
                .unmap(vaddr)
                .inspect_err(|e| error!("failed to unmap page: {vaddr_usize:#x?}, {e:?}"))?;
            if flush_tlb_by_page {
                tlb.flush();
            } else {
                tlb.ignore();
            }

            assert!(page_size.is_aligned(vaddr_usize));
            assert!(page_size as usize <= size);
            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(TlbFlushAll::new())
    }

    /// Updates mapping flags of a contiguous virtual memory region.
    pub fn protect_region(
        &mut self,
        vaddr: M::VirtAddr,
        size: usize,
        flags: MappingFlags,
        flush_tlb_by_page: bool,
    ) -> PagingResult<TlbFlushAll<M>> {
        let mut vaddr_usize: usize = vaddr.into();
        let mut size = size;
        trace!(
            "protect_region({:#x}) [{:#x}, {:#x}) {:?}",
            self.root_paddr(),
            vaddr_usize,
            vaddr_usize + size,
            flags,
        );
        while size > 0 {
            let vaddr = vaddr_usize.into();
            let (page_size, tlb) = self
                .protect(vaddr, flags)
                .inspect_err(|e| error!("failed to protect page: {vaddr_usize:#x?}, {e:?}"))?;
            if flush_tlb_by_page {
                tlb.flush();
            } else {
                tlb.ignore();
            }

            assert!(page_size.is_aligned(vaddr_usize));
            assert!(page_size as usize <= size);
            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(TlbFlushAll::new())
    }

    /// Copy entries from another page table within the given virtual memory range.
    #[cfg(feature = "copy-from")]
    pub fn copy_from(&mut self, other: &Self, start: M::VirtAddr, size: usize) {
        if size == 0 {
            return;
        }
        let src_table = self.get_table(other.root_paddr);
        let dst_table = self.get_table_mut(self.root_paddr);

        let start_idx = p1_index(start.into());
        let end_idx = p1_index(start.into() + size - 1) + 1;
        assert!(start_idx < ENTRY_COUNT);
        assert!(end_idx <= ENTRY_COUNT);

        for i in start_idx..end_idx {
            dst_table[i] = src_table[i];
        }
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler> Drop for PageTable32<M, PTE, H> {
    fn drop(&mut self) {
        // Deallocate all L2 page tables (each is 4KB)
        let table = self.get_table(self.root_paddr);
        for entry in table {
            if !entry.is_unused() && !entry.is_huge() {
                // This is an L2 page table (4KB)
                H::dealloc_frame(entry.paddr());
            }
        }
        // Deallocate L1 page table (16KB = 4 pages)
        H::dealloc_frame_contiguous(self.root_paddr, 4);
    }
}
