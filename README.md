# page_table

This crate provides generic, unified, architecture-independent, and OS-free page table structures for various hardware architectures.

The core struct is [`PageTable64<M, PTE, IF>`]. OS-functions and architecture-dependent types are provided by generic parameters:

- `M`: The architecture-dependent metadata, requires to implement the [`PagingMetaData`] trait.
- `PTE`: The architecture-dependent page table entry, requires to implement the [`GenericPTE`] trait.
- `IF`: OS-functions such as physical memory allocation, requires to implement the [`PagingIf`] trait.

Currently supported architectures and page table structures:

- x86: [`x86_64::X64PageTable`]
- ARM: [`aarch64::A64PageTable`]
- RISC-V: [`riscv::Sv39PageTable`], [`riscv::Sv48PageTable`]
