use crate::{GenericPTE, PagingHandler, PagingMetaData};
use crate::{MappingFlags, PageSize, PagingError, PagingResult, TlbFlush, TlbFlushAll};
use core::marker::PhantomData;
use memory_addr::{AddrRange, MemoryAddr, PAGE_SIZE_4K, PhysAddr};

const ENTRY_COUNT: usize = 512;

const P4E_ADDR_RANGE: usize = 1 << 39; // 512GB

const fn p4_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 27)) & (ENTRY_COUNT - 1)
}

const fn p3_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 18)) & (ENTRY_COUNT - 1)
}

const fn p2_index(vaddr: usize) -> usize {
    (vaddr >> (12 + 9)) & (ENTRY_COUNT - 1)
}

const fn p1_index(vaddr: usize) -> usize {
    (vaddr >> 12) & (ENTRY_COUNT - 1)
}

/// A generic page table struct for 64-bit platform.
///
/// It also tracks all intermediate level tables. They will be deallocated
/// When the [`EqPageTable64Ext`] itself is dropped.
pub struct EqPageTable64Ext<
    M: PagingMetaData,
    PTE: GenericPTE,
    H: PagingHandler,
    SH: PagingHandler = H,
> {
    root_paddr: PhysAddr,
    shared_vaddr_range: Option<AddrRange<M::VirtAddr>>,
    shared_vaddr_pgdir_initialized: bool,
    _phantom: PhantomData<(M, PTE, H, SH)>,
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler, SH: PagingHandler>
    EqPageTable64Ext<M, PTE, H, SH>
{
    // /// Creates a new page table instance or returns the error.
    // ///
    // /// It will allocate a new page for the root page table.
    // pub fn try_new() -> PagingResult<Self> {
    //     let root_paddr = Self::alloc_table()?;
    //     Ok(Self {
    //         root_paddr,
    //         shared_vaddr_range: None,
    //         _phantom: PhantomData,
    //     })
    // }

    pub fn from_paddr(
        root_paddr: PhysAddr,
        shared_vaddr_range: Option<AddrRange<M::VirtAddr>>,
    ) -> PagingResult<Self> {
        if let Some(range) = &shared_vaddr_range {
            if !range.start.is_aligned(P4E_ADDR_RANGE) || !range.end.is_aligned(P4E_ADDR_RANGE) {
                error!(
                    "shared_vaddr_range {:?} is not aligned to {:#x}",
                    range, P4E_ADDR_RANGE
                );
                return Err(PagingError::NotAligned);
            }
        }

        Ok(Self {
            root_paddr,
            shared_vaddr_range,
            shared_vaddr_pgdir_initialized: false,
            _phantom: PhantomData,
        })
    }

    /// Initialize the P4E entries for the shared virtual address range,
    /// this function should be called before forking this process.
    fn init_shared_vaddr_range_pgdir(&mut self) -> PagingResult<()> {
        let range = if let Some(range) = &self.shared_vaddr_range {
            range
        } else {
            return Ok(());
        };

        if M::LEVELS == 3 {
            error!(
                "shared_vaddr_range {:?} is not supported in 3-level page table",
                range
            );
            return Err(PagingError::NotAligned);
        }

        let start_vaddr = range.start;
        let end_vaddr = range.end;
        if !start_vaddr.is_aligned(P4E_ADDR_RANGE) || !end_vaddr.is_aligned(P4E_ADDR_RANGE) {
            error!(
                "shared_vaddr_range {:?} is not aligned to {:#x}",
                range, P4E_ADDR_RANGE
            );
            return Err(PagingError::NotAligned);
        }

        let mut vaddr = start_vaddr.into();

        while vaddr < end_vaddr.into() {
            let p4 = self.table_of_mut(self.root_paddr());
            let index = p4_index(vaddr);
            let p4e = &mut p4[index];

            // Prefill the P4E, allocate the physical frame for the next level page table.
            // Because the PGDIR frame will be copied during fork, while the next level
            // page table is shared among processes.
            let _p3e = self.next_table_mut_or_create(vaddr, p4e, true)?;

            info!("Prefilled P4E[{index}] for vaddr {:#x}", vaddr);

            vaddr = vaddr.add(P4E_ADDR_RANGE);
        }

        self.shared_vaddr_pgdir_initialized = true;

        Ok(())
    }

    /// Returns the physical address of the root page table.
    pub const fn root_paddr(&self) -> PhysAddr {
        self.root_paddr
    }

    /// Maps a virtual page to a physical frame with the given `page_size`
    /// and mapping `flags`.
    ///
    /// The virtual page starts with `vaddr`, amd the physical frame starts with
    /// `target`. If the addresses is not aligned to the page size, they will be
    /// aligned down automatically.
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

    /// Remap the mapping starts with `vaddr`, updates both the physical address
    /// and flags.
    ///
    /// Returns the page size of the mapping.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// intermediate level tables of the mapping is not present.
    pub fn remap(
        &mut self,
        vaddr: M::VirtAddr,
        paddr: PhysAddr,
        flags: MappingFlags,
    ) -> PagingResult<(PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        entry.set_paddr(paddr);
        entry.set_flags(flags, size.is_huge());
        Ok((size, TlbFlush::new(vaddr)))
    }

    /// Updates the flags of the mapping starts with `vaddr`.
    ///
    /// Returns the page size of the mapping.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn protect(
        &mut self,
        vaddr: M::VirtAddr,
        flags: MappingFlags,
    ) -> PagingResult<(PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if !entry.is_present() {
            return Err(PagingError::NotMapped);
        }
        entry.set_flags(flags, size.is_huge());
        Ok((size, TlbFlush::new(vaddr)))
    }

    /// Unmaps the mapping starts with `vaddr`.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn unmap(&mut self, vaddr: M::VirtAddr) -> PagingResult<(PhysAddr, PageSize, TlbFlush<M>)> {
        let (entry, size) = self.get_entry_mut(vaddr)?;
        if !entry.is_present() {
            entry.clear();
            return Err(PagingError::NotMapped);
        }
        let paddr = entry.paddr();
        entry.clear();
        Ok((paddr, size, TlbFlush::new(vaddr)))
    }

    /// Queries the result of the mapping starts with `vaddr`.
    ///
    /// Returns the physical address of the target frame, mapping flags, and
    /// the page size.
    ///
    /// Returns [`Err(PagingError::NotMapped)`](PagingError::NotMapped) if the
    /// mapping is not present.
    pub fn query(&self, vaddr: M::VirtAddr) -> PagingResult<(PhysAddr, MappingFlags, PageSize)> {
        let (entry, size) = self.get_entry(vaddr)?;
        if entry.is_unused() {
            return Err(PagingError::NotMapped);
        }
        let off = size.align_offset(vaddr.into());
        Ok((entry.paddr().add(off), entry.flags(), size))
    }

    /// Maps a contiguous virtual memory region to a contiguous physical memory
    /// region with the given mapping `flags`.
    ///
    /// The virtual and physical memory regions start with `vaddr` and `paddr`
    /// respectively. The region size is `size`. The addresses and `size` must
    /// be aligned to 4K, otherwise it will return [`Err(PagingError::NotAligned)`].
    ///
    /// When `allow_huge` is true, it will try to map the region with huge pages
    /// if possible. Otherwise, it will map the region with 4K pages.
    ///
    /// When `flush_tlb_by_page` is true, it will flush the TLB immediately after
    /// mapping each page. Otherwise, the TLB flush should by handled by the caller.
    ///
    /// [`Err(PagingError::NotAligned)`]: PagingError::NotAligned
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
            let page_size = if allow_huge {
                if PageSize::Size1G.is_aligned(vaddr_usize)
                    && paddr.is_aligned(PageSize::Size1G)
                    && size >= PageSize::Size1G as usize
                {
                    PageSize::Size1G
                } else if PageSize::Size2M.is_aligned(vaddr_usize)
                    && paddr.is_aligned(PageSize::Size2M)
                    && size >= PageSize::Size2M as usize
                {
                    PageSize::Size2M
                } else {
                    PageSize::Size4K
                }
            } else {
                PageSize::Size4K
            };
            let tlb = self.map(vaddr, paddr, page_size, flags).inspect_err(|e| {
                error!(
                    "failed to map page: {:#x?}({:?}) -> {:#x?}, {:?}",
                    vaddr_usize, page_size, paddr, e
                )
            })?;
            if flush_tlb_by_page {
                M::flush_tlb(Some(vaddr));
            }
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
    ///
    /// The region must be mapped before using [`PageTable64::map_region`], or
    /// unexpected behaviors may occur. It can deal with huge pages automatically.
    ///
    /// When `flush_tlb_by_page` is true, it will flush the TLB immediately after
    /// mapping each page. Otherwise, the TLB flush should by handled by the caller.
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
                .inspect_err(|e| error!("failed to unmap page: {:#x?}, {:?}", vaddr_usize, e))?;
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
    ///
    /// The region must be mapped before using [`PageTable64::map_region`], or
    /// unexpected behaviors may occur. It can deal with huge pages automatically.
    ///
    /// When `flush_tlb_by_page` is true, it will flush the TLB immediately after
    /// mapping each page. Otherwise, the TLB flush should by handled by the caller.
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
            let page_size = match self.protect(vaddr, flags) {
                Ok((page_size, tlb)) => {
                    if flush_tlb_by_page {
                        tlb.flush();
                    } else {
                        tlb.ignore();
                    }
                    page_size
                }
                // Allow skipping unmapped pages.
                Err(PagingError::NotMapped) => PageSize::Size4K,
                Err(e) => {
                    error!("failed to protect page: {:#x?}, {:?}", vaddr_usize, e);
                    return Err(e);
                }
            };

            // let (page_size, tlb) = self
            //     .protect(vaddr, flags)
            //     .inspect_err(|e| error!("failed to protect page: {:#x?}, {:?}", vaddr_usize, e))?;

            assert!(page_size.is_aligned(vaddr_usize));
            assert!(page_size as usize <= size);
            vaddr_usize += page_size as usize;
            size -= page_size as usize;
        }
        Ok(TlbFlushAll::new())
    }

    /// Walk the page table recursively.
    ///
    /// When reaching a page table entry, call `pre_func` and `post_func` on the
    /// entry if they are provided. The max number of enumerations in one table
    /// is limited by `limit`. `pre_func` and `post_func` are called before and
    /// after recursively walking the page table.
    ///
    /// The arguments of `*_func` are:
    /// - Current level (starts with `0`): `usize`
    /// - The index of the entry in the current-level table: `usize`
    /// - The virtual address that is mapped to the entry: `M::VirtAddr`
    /// - The reference of the entry: [`&PTE`](GenericPTE)
    pub fn walk<F>(&self, limit: usize, pre_func: Option<&F>, post_func: Option<&F>) -> PagingResult
    where
        F: Fn(usize, usize, M::VirtAddr, &PTE),
    {
        self.walk_recursive(
            self.table_of(self.root_paddr()),
            0,
            0.into(),
            limit,
            pre_func,
            post_func,
        )
    }

    /// Copy entries from another page table within the given virtual memory range.
    pub fn copy_from(&mut self, other: &Self, start: M::VirtAddr, size: usize) {
        if size == 0 {
            return;
        }
        let src_table = self.table_of(other.root_paddr);
        let dst_table = self.table_of_mut(self.root_paddr);
        let index_fn = if M::LEVELS == 3 {
            p3_index
        } else if M::LEVELS == 4 {
            p4_index
        } else {
            unreachable!()
        };
        let start_idx = index_fn(start.into());
        let end_idx = index_fn(start.into() + size - 1) + 1;
        assert!(start_idx < ENTRY_COUNT);
        assert!(end_idx <= ENTRY_COUNT);
        dst_table[start_idx..end_idx].copy_from_slice(&src_table[start_idx..end_idx]);
    }
}

// Private implements.
impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler, SH: PagingHandler>
    EqPageTable64Ext<M, PTE, H, SH>
{
    fn alloc_table(shared_pt: bool) -> PagingResult<PhysAddr> {
        let allocated_pt_frame = if shared_pt {
            SH::alloc_frame()
        } else {
            H::alloc_frame()
        };

        if let Some(paddr) = allocated_pt_frame {
            let ptr = H::phys_to_virt(paddr).as_mut_ptr();
            unsafe { core::ptr::write_bytes(ptr, 0, PAGE_SIZE_4K) };
            Ok(paddr)
        } else {
            Err(PagingError::NoMemory)
        }
    }

    fn table_of<'a>(&self, paddr: PhysAddr) -> &'a [PTE] {
        let ptr = H::phys_to_virt(paddr).as_ptr() as _;
        unsafe { core::slice::from_raw_parts(ptr, ENTRY_COUNT) }
    }

    fn table_of_mut<'a>(&mut self, paddr: PhysAddr) -> &'a mut [PTE] {
        let ptr = H::phys_to_virt(paddr).as_mut_ptr() as _;
        unsafe { core::slice::from_raw_parts_mut(ptr, ENTRY_COUNT) }
    }

    fn next_table<'a>(&self, entry: &PTE) -> PagingResult<&'a [PTE]> {
        if entry.paddr().as_usize() == 0 {
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            Err(PagingError::MappedToHugePage)
        } else {
            Ok(self.table_of(entry.paddr()))
        }
    }

    fn next_table_mut<'a>(&mut self, entry: &PTE) -> PagingResult<&'a mut [PTE]> {
        if entry.paddr().as_usize() == 0 {
            Err(PagingError::NotMapped)
        } else if entry.is_huge() {
            Err(PagingError::MappedToHugePage)
        } else {
            Ok(self.table_of_mut(entry.paddr()))
        }
    }

    fn next_table_mut_or_create<'a>(
        &mut self,
        vaddr: usize, // Just for debug info.
        entry: &mut PTE,
        shared_pt: bool,
    ) -> PagingResult<&'a mut [PTE]> {
        if entry.is_unused() {
            let paddr = Self::alloc_table(shared_pt)?;
            if !shared_pt {
                trace!("allow pt frame for vaddr {:#x} at {:?}", vaddr, paddr);
            }
            *entry = GenericPTE::new_table(paddr);
            Ok(self.table_of_mut(paddr))
        } else {
            self.next_table_mut(entry)
        }
    }

    fn get_entry(&self, vaddr: M::VirtAddr) -> PagingResult<(&PTE, PageSize)> {
        let vaddr: usize = vaddr.into();
        let p3 = if M::LEVELS == 3 {
            self.table_of(self.root_paddr())
        } else if M::LEVELS == 4 {
            let p4 = self.table_of(self.root_paddr());
            let p4e = &p4[p4_index(vaddr)];
            self.next_table(p4e)?
        } else {
            unreachable!()
        };
        let p3e = &p3[p3_index(vaddr)];
        if p3e.is_huge() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = self.next_table(p3e)?;
        let p2e = &p2[p2_index(vaddr)];
        if p2e.is_huge() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = self.next_table(p2e)?;
        let p1e = &p1[p1_index(vaddr)];
        Ok((p1e, PageSize::Size4K))
    }

    fn get_entry_mut(&mut self, vaddr: M::VirtAddr) -> PagingResult<(&mut PTE, PageSize)> {
        let vaddr: usize = vaddr.into();
        let p3 = if M::LEVELS == 3 {
            self.table_of_mut(self.root_paddr())
        } else if M::LEVELS == 4 {
            let p4 = self.table_of_mut(self.root_paddr());
            let p4e = &mut p4[p4_index(vaddr)];
            self.next_table_mut(p4e)?
        } else {
            unreachable!()
        };
        let p3e = &mut p3[p3_index(vaddr)];
        if p3e.is_huge() {
            return Ok((p3e, PageSize::Size1G));
        }

        let p2 = self.next_table_mut(p3e)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if p2e.is_huge() {
            return Ok((p2e, PageSize::Size2M));
        }

        let p1 = self.next_table_mut(p2e)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok((p1e, PageSize::Size4K))
    }

    fn get_entry_mut_or_create(
        &mut self,
        vaddr: M::VirtAddr,
        page_size: PageSize,
    ) -> PagingResult<&mut PTE> {
        let shared_pt = if let Some(range) = &self.shared_vaddr_range {
            // These address are total hack, should be defined in a better way.
            // 0x5000_0000_0000: `USER_MMAP_BASE_HINT`, Junction will only allocate address below this for user,
            // 0x200000: the low memory region used by Junction.
            vaddr.into() >= 0x5000_0000_0000 || range.contains(vaddr) || vaddr.into() < 0x200000
        } else {
            false
        };

        if shared_pt && !self.shared_vaddr_pgdir_initialized {
            self.init_shared_vaddr_range_pgdir()?;
        }
        let vaddr: usize = vaddr.into();

        // if shared_pt {
        //     info!("vaddr: {:#x?} is in shared pt", vaddr);
        // }
        let p3 = if M::LEVELS == 3 {
            self.table_of_mut(self.root_paddr())
        } else if M::LEVELS == 4 {
            let p4 = self.table_of_mut(self.root_paddr());
            let p4e = &mut p4[p4_index(vaddr)];
            self.next_table_mut_or_create(vaddr, p4e, shared_pt)?
        } else {
            unreachable!()
        };
        let p3e = &mut p3[p3_index(vaddr)];
        if page_size == PageSize::Size1G {
            return Ok(p3e);
        }

        let p2 = self.next_table_mut_or_create(vaddr, p3e, shared_pt)?;
        let p2e = &mut p2[p2_index(vaddr)];
        if page_size == PageSize::Size2M {
            return Ok(p2e);
        }

        let p1 = self.next_table_mut_or_create(vaddr, p2e, shared_pt)?;
        let p1e = &mut p1[p1_index(vaddr)];
        Ok(p1e)
    }

    fn walk_recursive<F>(
        &self,
        table: &[PTE],
        level: usize,
        start_vaddr: M::VirtAddr,
        limit: usize,
        pre_func: Option<&F>,
        post_func: Option<&F>,
    ) -> PagingResult
    where
        F: Fn(usize, usize, M::VirtAddr, &PTE),
    {
        let start_vaddr_usize: usize = start_vaddr.into();
        let mut n = 0;
        for (i, entry) in table.iter().enumerate() {
            let vaddr_usize = start_vaddr_usize + (i << (12 + (M::LEVELS - 1 - level) * 9));
            let vaddr = vaddr_usize.into();

            if entry.is_present() {
                if let Some(func) = pre_func {
                    func(level, i, vaddr, entry);
                }
                if level < M::LEVELS - 1 && !entry.is_huge() {
                    let table_entry = self.next_table(entry)?;
                    self.walk_recursive(table_entry, level + 1, vaddr, limit, pre_func, post_func)?;
                }
                if let Some(func) = post_func {
                    func(level, i, vaddr, entry);
                }
                n += 1;
                if n >= limit {
                    break;
                }
            }
        }
        Ok(())
    }
}

impl<M: PagingMetaData, PTE: GenericPTE, H: PagingHandler, SH: PagingHandler> Drop
    for EqPageTable64Ext<M, PTE, H, SH>
{
    fn drop(&mut self) {
        // warn!("Dropping page table @ {:#x}", self.root_paddr());

        // don't free the entries in last level, they are not array.
        let _ = self.walk(
            usize::MAX,
            None,
            Some(&|level, _index, _vaddr, entry: &PTE| {
                if level < M::LEVELS - 1 && entry.is_present() && !entry.is_huge() {
                    H::dealloc_frame(entry.paddr());
                }
            }),
        );
        H::dealloc_frame(self.root_paddr());
    }
}
